//! IDE communication protocol.
//!
//! Defines the JSON-based message protocol used for bidirectional
//! communication between Claude Code and IDE extensions. Messages
//! are exchanged over WebSocket or SSE transport via the MCP layer.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Outgoing messages (Claude Code → IDE)
// ---------------------------------------------------------------------------

/// A message sent from Claude Code to the IDE extension.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OutgoingIdeMessage {
    /// Open a file in the IDE at an optional location.
    #[serde(rename = "openFile")]
    OpenFile {
        /// Absolute path to the file to open.
        path: String,
        /// Line number to scroll to (1-based).
        #[serde(skip_serializing_if = "Option::is_none")]
        line: Option<u32>,
        /// Column number to place the cursor at (1-based).
        #[serde(skip_serializing_if = "Option::is_none")]
        column: Option<u32>,
    },

    /// Show a diff between two file contents.
    #[serde(rename = "showDiff")]
    ShowDiff {
        /// Absolute path to the file.
        path: String,
        /// Original content (left side).
        original: String,
        /// Modified content (right side).
        modified: String,
        /// Optional title for the diff tab.
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },

    /// Request the current selection from the IDE.
    #[serde(rename = "getSelection")]
    GetSelection,

    /// Request the list of open files.
    #[serde(rename = "getOpenFiles")]
    GetOpenFiles,

    /// Request diagnostics (errors/warnings) for a file.
    #[serde(rename = "getDiagnostics")]
    GetDiagnostics {
        /// Absolute path to query diagnostics for.
        path: String,
    },

    /// Show an inline notification in the IDE.
    #[serde(rename = "showNotification")]
    ShowNotification {
        /// Notification message text.
        message: String,
        /// Severity: `"info"`, `"warning"`, or `"error"`.
        #[serde(default = "default_info")]
        level: String,
    },

    /// Request workspace folder information.
    #[serde(rename = "getWorkspaceFolders")]
    GetWorkspaceFolders,
}

fn default_info() -> String {
    "info".to_string()
}

// ---------------------------------------------------------------------------
// Incoming messages (IDE → Claude Code)
// ---------------------------------------------------------------------------

/// A message received from the IDE extension.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IncomingIdeMessage {
    /// Response to a [`OutgoingIdeMessage::GetSelection`] request.
    #[serde(rename = "selection")]
    Selection {
        /// Path to the file containing the selection.
        path: String,
        /// The selected text.
        text: String,
        /// Starting line (1-based).
        start_line: u32,
        /// Ending line (1-based).
        end_line: u32,
    },

    /// Response to a [`OutgoingIdeMessage::GetOpenFiles`] request.
    #[serde(rename = "openFiles")]
    OpenFiles {
        /// List of absolute paths to currently open files.
        files: Vec<String>,
    },

    /// Response to a [`OutgoingIdeMessage::GetDiagnostics`] request.
    #[serde(rename = "diagnostics")]
    Diagnostics {
        /// The file path these diagnostics belong to.
        path: String,
        /// List of diagnostic entries.
        diagnostics: Vec<IdeDiagnostic>,
    },

    /// Response to a [`OutgoingIdeMessage::GetWorkspaceFolders`] request.
    #[serde(rename = "workspaceFolders")]
    WorkspaceFolders {
        /// List of workspace folder paths.
        folders: Vec<String>,
    },

    /// A file was saved in the IDE (notification, not a request response).
    #[serde(rename = "fileSaved")]
    FileSaved {
        /// Absolute path to the saved file.
        path: String,
    },

    /// A file was opened in the IDE.
    #[serde(rename = "fileOpened")]
    FileOpened {
        /// Absolute path to the opened file.
        path: String,
    },

    /// The active editor changed in the IDE.
    #[serde(rename = "activeEditorChanged")]
    ActiveEditorChanged {
        /// Absolute path to the now-active file.
        path: String,
    },

    /// Generic error response from the IDE.
    #[serde(rename = "error")]
    Error {
        /// Error message.
        message: String,
        /// Original request ID this error corresponds to.
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },
}

/// A diagnostic entry (error, warning, etc.) from the IDE.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IdeDiagnostic {
    /// Severity: `"error"`, `"warning"`, `"info"`, `"hint"`.
    pub severity: String,
    /// Diagnostic message text.
    pub message: String,
    /// Source of the diagnostic (e.g. "rust-analyzer", "eslint").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Starting line (1-based).
    pub start_line: u32,
    /// Starting column (1-based).
    pub start_column: u32,
    /// Ending line (1-based).
    pub end_line: u32,
    /// Ending column (1-based).
    pub end_column: u32,
}

// ---------------------------------------------------------------------------
// RPC wrapper for MCP-based communication
// ---------------------------------------------------------------------------

/// An RPC request sent over MCP to the IDE extension.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IdeRpcRequest {
    /// Unique request identifier for correlating responses.
    pub id: String,
    /// The method name (maps to [`OutgoingIdeMessage`] type).
    pub method: String,
    /// Parameters for the request.
    #[serde(default)]
    pub params: Value,
}

/// An RPC response received from the IDE extension.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IdeRpcResponse {
    /// Matches the request ID.
    pub id: String,
    /// Result payload on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error message on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl IdeRpcRequest {
    /// Create a new RPC request with a random ID.
    pub fn new(method: &str, params: Value) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            method: method.to_string(),
            params,
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
    fn test_outgoing_open_file_serialization() {
        let msg = OutgoingIdeMessage::OpenFile {
            path: "/src/main.rs".to_string(),
            line: Some(42),
            column: None,
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "openFile");
        assert_eq!(json["path"], "/src/main.rs");
        assert_eq!(json["line"], 42);
        assert!(json.get("column").is_none());
    }

    #[test]
    fn test_outgoing_show_diff_serialization() {
        let msg = OutgoingIdeMessage::ShowDiff {
            path: "/src/lib.rs".to_string(),
            original: "old code".to_string(),
            modified: "new code".to_string(),
            title: Some("Edit".to_string()),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "showDiff");
        assert_eq!(json["original"], "old code");
        assert_eq!(json["modified"], "new code");
    }

    #[test]
    fn test_incoming_selection_deserialization() {
        let json = json!({
            "type": "selection",
            "path": "/src/main.rs",
            "text": "fn main()",
            "start_line": 1,
            "end_line": 1
        });
        let msg: IncomingIdeMessage = serde_json::from_value(json).unwrap();
        match msg {
            IncomingIdeMessage::Selection { path, text, .. } => {
                assert_eq!(path, "/src/main.rs");
                assert_eq!(text, "fn main()");
            }
            _ => panic!("expected Selection variant"),
        }
    }

    #[test]
    fn test_incoming_diagnostics_deserialization() {
        let json = json!({
            "type": "diagnostics",
            "path": "/src/lib.rs",
            "diagnostics": [{
                "severity": "error",
                "message": "mismatched types",
                "source": "rust-analyzer",
                "start_line": 10,
                "start_column": 5,
                "end_line": 10,
                "end_column": 20
            }]
        });
        let msg: IncomingIdeMessage = serde_json::from_value(json).unwrap();
        match msg {
            IncomingIdeMessage::Diagnostics { diagnostics, .. } => {
                assert_eq!(diagnostics.len(), 1);
                assert_eq!(diagnostics[0].severity, "error");
            }
            _ => panic!("expected Diagnostics variant"),
        }
    }

    #[test]
    fn test_ide_rpc_request() {
        let req = IdeRpcRequest::new("openFile", json!({"path": "/test.rs"}));
        assert_eq!(req.method, "openFile");
        assert!(!req.id.is_empty());
    }

    #[test]
    fn test_ide_rpc_response_serialization() {
        let resp = IdeRpcResponse {
            id: "req-1".to_string(),
            result: Some(json!({"files": ["/a.rs"]})),
            error: None,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["id"], "req-1");
        assert!(json.get("error").is_none());
    }
}
