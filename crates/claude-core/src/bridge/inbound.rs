//! Inbound message and attachment handling for the bridge.
//!
//! Processes inbound user messages from the server, including:
//! - Content extraction and normalization
//! - Image block normalization (camelCase -> snake_case media_type)
//! - File attachment resolution (download + local write + @path refs)
//! - Content validation

use std::path::{Path, PathBuf};

use serde_json::Value;

// ---------------------------------------------------------------------------
// Inbound message extraction
// ---------------------------------------------------------------------------

/// Extracted fields from an inbound user message.
pub struct InboundMessageFields {
    /// The message content (either a string or array of content blocks).
    pub content: InboundContent,
    /// The message UUID, if present.
    pub uuid: Option<String>,
}

/// Content of an inbound message.
#[derive(Clone, Debug)]
pub enum InboundContent {
    /// Plain text content.
    Text(String),
    /// Array of content blocks (text, images, etc.).
    Blocks(Vec<Value>),
}

impl InboundContent {
    /// Get the plain text content, if this is a text message.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            InboundContent::Text(s) => Some(s),
            InboundContent::Blocks(_) => None,
        }
    }

    /// Check if the content is empty.
    pub fn is_empty(&self) -> bool {
        match self {
            InboundContent::Text(s) => s.is_empty(),
            InboundContent::Blocks(b) => b.is_empty(),
        }
    }
}

/// Process an inbound user message from the bridge, extracting content
/// and UUID for enqueueing.
///
/// Normalizes image blocks from bridge clients that may use camelCase
/// `mediaType` instead of snake_case `media_type`.
///
/// Returns `None` if the message should be skipped (non-user type,
/// missing/empty content).
pub fn extract_inbound_message_fields(msg: &Value) -> Option<InboundMessageFields> {
    let obj = msg.as_object()?;

    // Only process user messages
    if obj.get("type")?.as_str()? != "user" {
        return None;
    }

    let message = obj.get("message")?.as_object()?;
    let content_val = message.get("content")?;

    let content = if let Some(s) = content_val.as_str() {
        if s.is_empty() {
            return None;
        }
        InboundContent::Text(s.to_string())
    } else if let Some(arr) = content_val.as_array() {
        if arr.is_empty() {
            return None;
        }
        InboundContent::Blocks(normalize_image_blocks(arr))
    } else {
        return None;
    };

    let uuid = obj
        .get("uuid")
        .and_then(|u| u.as_str())
        .map(|s| s.to_string());

    Some(InboundMessageFields { content, uuid })
}

// ---------------------------------------------------------------------------
// Image block normalization
// ---------------------------------------------------------------------------

/// Check if an image block is malformed (missing snake_case `media_type`).
fn is_malformed_base64_image(block: &Value) -> bool {
    let obj = match block.as_object() {
        Some(o) => o,
        None => return false,
    };

    if obj.get("type").and_then(|t| t.as_str()) != Some("image") {
        return false;
    }

    let source = match obj.get("source").and_then(|s| s.as_object()) {
        Some(s) => s,
        None => return false,
    };

    if source.get("type").and_then(|t| t.as_str()) != Some("base64") {
        return false;
    }

    // Missing snake_case media_type
    source.get("media_type").is_none()
}

/// Normalize image content blocks from bridge clients.
///
/// iOS/web clients may send `mediaType` (camelCase) instead of `media_type`
/// (snake_case), or omit the field entirely. Without normalization, the bad
/// block poisons the session.
///
/// Fast-path returns the original array when no normalization is needed
/// (zero allocation on the happy path).
fn normalize_image_blocks(blocks: &[Value]) -> Vec<Value> {
    if !blocks.iter().any(is_malformed_base64_image) {
        return blocks.to_vec();
    }

    blocks
        .iter()
        .map(|block| {
            if !is_malformed_base64_image(block) {
                return block.clone();
            }

            let obj = block.as_object().unwrap();
            let source = obj.get("source").and_then(|s| s.as_object()).unwrap();

            // Try camelCase mediaType first, then attempt detection from data
            let media_type = source
                .get("mediaType")
                .and_then(|m| m.as_str())
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| {
                    // Attempt to detect from base64 data prefix
                    let data = source
                        .get("data")
                        .and_then(|d| d.as_str())
                        .unwrap_or("");
                    detect_image_format_from_base64(data)
                });

            let data = source
                .get("data")
                .cloned()
                .unwrap_or(Value::String(String::new()));

            let mut normalized = obj.clone();
            normalized.insert(
                "source".to_string(),
                serde_json::json!({
                    "type": "base64",
                    "media_type": media_type,
                    "data": data,
                }),
            );

            Value::Object(normalized)
        })
        .collect()
}

/// Detect image format from the first few bytes of base64-encoded data.
fn detect_image_format_from_base64(data: &str) -> &'static str {
    // PNG: starts with iVBOR
    if data.starts_with("iVBOR") {
        return "image/png";
    }
    // JPEG: starts with /9j/
    if data.starts_with("/9j/") {
        return "image/jpeg";
    }
    // GIF: starts with R0lG
    if data.starts_with("R0lG") {
        return "image/gif";
    }
    // WebP: starts with UklG
    if data.starts_with("UklG") {
        return "image/webp";
    }
    // Default to PNG
    "image/png"
}

// ---------------------------------------------------------------------------
// Inbound attachments
// ---------------------------------------------------------------------------

/// An inbound file attachment from a bridge message.
#[derive(Clone, Debug)]
pub struct InboundAttachment {
    /// Server-assigned file UUID.
    pub file_uuid: String,
    /// Original file name.
    pub file_name: String,
}

/// Extract file attachments from an inbound message.
pub fn extract_inbound_attachments(msg: &Value) -> Vec<InboundAttachment> {
    let attachments = match msg.get("file_attachments").and_then(|a| a.as_array()) {
        Some(arr) => arr,
        None => return Vec::new(),
    };

    attachments
        .iter()
        .filter_map(|att| {
            let obj = att.as_object()?;
            let file_uuid = obj.get("file_uuid")?.as_str()?.to_string();
            let file_name = obj.get("file_name")?.as_str()?.to_string();
            Some(InboundAttachment {
                file_uuid,
                file_name,
            })
        })
        .collect()
}

/// Sanitize a file name for safe filesystem use.
///
/// Strips path components and keeps only filename-safe characters.
fn sanitize_file_name(name: &str) -> String {
    let base = Path::new(name)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_string());

    let sanitized: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();

    if sanitized.is_empty() {
        "attachment".to_string()
    } else {
        sanitized
    }
}

/// Resolve a single attachment by downloading it and writing to disk.
///
/// Returns the absolute file path on success, `None` on any failure.
pub async fn resolve_attachment(
    att: &InboundAttachment,
    base_url: &str,
    access_token: &str,
    uploads_dir: &Path,
) -> Option<PathBuf> {
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .ok()?;

    // Simple percent-encoding for UUID characters (safe for alphanumeric + hyphens)
    let encoded_uuid: String = att
        .file_uuid
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c.to_string()
            } else {
                format!("%{:02X}", c as u8)
            }
        })
        .collect();
    let url = format!(
        "{}/api/oauth/files/{}/content",
        base_url, encoded_uuid
    );

    let resp = http
        .get(&url)
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .await
        .ok()?;

    if resp.status().as_u16() != 200 {
        tracing::debug!(
            "[bridge:inbound-attach] fetch {} failed: status={}",
            att.file_uuid,
            resp.status()
        );
        return None;
    }

    let data = resp.bytes().await.ok()?;

    let safe_name = sanitize_file_name(&att.file_name);
    let prefix = att.file_uuid.get(..8).unwrap_or("unknown_");
    let prefix: String = prefix
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();

    let out_path = uploads_dir.join(format!("{prefix}-{safe_name}"));

    tokio::fs::create_dir_all(uploads_dir).await.ok()?;
    tokio::fs::write(&out_path, &data).await.ok()?;

    tracing::debug!(
        "[bridge:inbound-attach] resolved {} -> {} ({} bytes)",
        att.file_uuid,
        out_path.display(),
        data.len()
    );

    Some(out_path)
}

/// Resolve all attachments and build an @path reference prefix string.
///
/// Returns an empty string if no attachments were resolved.
pub async fn resolve_inbound_attachments(
    attachments: &[InboundAttachment],
    base_url: &str,
    access_token: &str,
    uploads_dir: &Path,
) -> String {
    if attachments.is_empty() {
        return String::new();
    }

    let mut paths = Vec::new();
    for att in attachments {
        if let Some(path) = resolve_attachment(att, base_url, access_token, uploads_dir).await {
            paths.push(path);
        }
    }

    if paths.is_empty() {
        return String::new();
    }

    // Build @"path" refs (quoted form handles spaces in paths)
    let refs: Vec<String> = paths
        .iter()
        .map(|p| format!("@\"{}\"", p.display()))
        .collect();
    format!("{} ", refs.join(" "))
}

/// Prepend @path references to content (string or content blocks).
pub fn prepend_path_refs(content: &InboundContent, prefix: &str) -> InboundContent {
    if prefix.is_empty() {
        return content.clone();
    }

    match content {
        InboundContent::Text(s) => InboundContent::Text(format!("{prefix}{s}")),
        InboundContent::Blocks(blocks) => {
            // Find the last text block and prepend the refs
            let last_text_idx = blocks
                .iter()
                .rposition(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"));

            if let Some(idx) = last_text_idx {
                let mut new_blocks = blocks.clone();
                if let Some(text) = new_blocks[idx].get("text").and_then(|t| t.as_str()) {
                    let mut block = new_blocks[idx].as_object().unwrap().clone();
                    block.insert(
                        "text".to_string(),
                        Value::String(format!("{prefix}{text}")),
                    );
                    new_blocks[idx] = Value::Object(block);
                }
                InboundContent::Blocks(new_blocks)
            } else {
                // No text block -- append one
                let mut new_blocks = blocks.clone();
                new_blocks.push(serde_json::json!({
                    "type": "text",
                    "text": prefix.trim_end(),
                }));
                InboundContent::Blocks(new_blocks)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract_inbound_message_fields_text() {
        let msg = json!({
            "type": "user",
            "uuid": "msg-uuid-1",
            "message": {"content": "Hello world"}
        });
        let fields = extract_inbound_message_fields(&msg).unwrap();
        assert_eq!(fields.content.as_text(), Some("Hello world"));
        assert_eq!(fields.uuid, Some("msg-uuid-1".to_string()));
    }

    #[test]
    fn test_extract_inbound_message_fields_blocks() {
        let msg = json!({
            "type": "user",
            "message": {
                "content": [
                    {"type": "text", "text": "Look at this:"},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBOR..."}}
                ]
            }
        });
        let fields = extract_inbound_message_fields(&msg).unwrap();
        assert!(matches!(fields.content, InboundContent::Blocks(_)));
        assert!(fields.uuid.is_none());
    }

    #[test]
    fn test_extract_inbound_message_fields_non_user() {
        let msg = json!({"type": "assistant", "message": {"content": "hi"}});
        assert!(extract_inbound_message_fields(&msg).is_none());
    }

    #[test]
    fn test_extract_inbound_message_fields_empty_content() {
        let msg = json!({"type": "user", "message": {"content": ""}});
        assert!(extract_inbound_message_fields(&msg).is_none());

        let msg = json!({"type": "user", "message": {"content": []}});
        assert!(extract_inbound_message_fields(&msg).is_none());
    }

    #[test]
    fn test_normalize_image_blocks_no_change() {
        let blocks = vec![
            json!({"type": "text", "text": "hello"}),
            json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "abc"}}),
        ];
        let result = normalize_image_blocks(&blocks);
        assert_eq!(result.len(), 2);
        // Should be the same content
        assert_eq!(result[1]["source"]["media_type"], "image/png");
    }

    #[test]
    fn test_normalize_image_blocks_camel_case() {
        let blocks = vec![json!({
            "type": "image",
            "source": {
                "type": "base64",
                "mediaType": "image/jpeg",
                "data": "/9j/test"
            }
        })];
        let result = normalize_image_blocks(&blocks);
        assert_eq!(result[0]["source"]["media_type"], "image/jpeg");
    }

    #[test]
    fn test_normalize_image_blocks_detect_format() {
        let blocks = vec![json!({
            "type": "image",
            "source": {
                "type": "base64",
                "data": "iVBORw0KGgo..."
            }
        })];
        let result = normalize_image_blocks(&blocks);
        assert_eq!(result[0]["source"]["media_type"], "image/png");
    }

    #[test]
    fn test_detect_image_format() {
        assert_eq!(detect_image_format_from_base64("iVBORw0K"), "image/png");
        assert_eq!(detect_image_format_from_base64("/9j/4AAQ"), "image/jpeg");
        assert_eq!(detect_image_format_from_base64("R0lGODlh"), "image/gif");
        assert_eq!(detect_image_format_from_base64("UklGRjAA"), "image/webp");
        assert_eq!(detect_image_format_from_base64("unknown"), "image/png");
    }

    #[test]
    fn test_extract_inbound_attachments() {
        let msg = json!({
            "file_attachments": [
                {"file_uuid": "uuid-1", "file_name": "test.png"},
                {"file_uuid": "uuid-2", "file_name": "doc.pdf"}
            ]
        });
        let attachments = extract_inbound_attachments(&msg);
        assert_eq!(attachments.len(), 2);
        assert_eq!(attachments[0].file_uuid, "uuid-1");
        assert_eq!(attachments[1].file_name, "doc.pdf");
    }

    #[test]
    fn test_extract_inbound_attachments_missing() {
        let msg = json!({"type": "user"});
        assert!(extract_inbound_attachments(&msg).is_empty());
    }

    #[test]
    fn test_sanitize_file_name() {
        assert_eq!(sanitize_file_name("test.png"), "test.png");
        assert_eq!(sanitize_file_name("../../../etc/passwd"), "passwd");
        assert_eq!(sanitize_file_name("file with spaces.txt"), "file_with_spaces.txt");
        assert_eq!(sanitize_file_name(""), "attachment");
    }

    #[test]
    fn test_prepend_path_refs_text() {
        let content = InboundContent::Text("Hello".to_string());
        let result = prepend_path_refs(&content, "@\"path/to/file\" ");
        assert_eq!(result.as_text(), Some("@\"path/to/file\" Hello"));
    }

    #[test]
    fn test_prepend_path_refs_blocks() {
        let content = InboundContent::Blocks(vec![
            json!({"type": "image", "source": {"type": "base64"}}),
            json!({"type": "text", "text": "Describe this"}),
        ]);
        let result = prepend_path_refs(&content, "@\"file.png\" ");
        if let InboundContent::Blocks(blocks) = result {
            assert_eq!(
                blocks[1]["text"].as_str().unwrap(),
                "@\"file.png\" Describe this"
            );
        } else {
            panic!("expected blocks");
        }
    }

    #[test]
    fn test_prepend_path_refs_no_text_block() {
        let content = InboundContent::Blocks(vec![
            json!({"type": "image", "source": {"type": "base64"}}),
        ]);
        let result = prepend_path_refs(&content, "@\"file.png\" ");
        if let InboundContent::Blocks(blocks) = result {
            assert_eq!(blocks.len(), 2);
            assert_eq!(blocks[1]["type"], "text");
        } else {
            panic!("expected blocks");
        }
    }

    #[test]
    fn test_prepend_path_refs_empty_prefix() {
        let content = InboundContent::Text("Hello".to_string());
        let result = prepend_path_refs(&content, "");
        assert_eq!(result.as_text(), Some("Hello"));
    }
}
