//! Model capability matrix, aliases, validation, and resolution utilities.

use std::env;

// ---------------------------------------------------------------------------
// Model string constants
// ---------------------------------------------------------------------------

// Model IDs match the original Claude Code `configs.ts` firstParty strings.
// The API accepts these short-form IDs without date suffixes.
pub const OPUS_46: &str = "claude-opus-4-6";
pub const OPUS_45: &str = "claude-opus-4-5-20250514";
pub const OPUS_41: &str = "claude-opus-4-1-20250805";
pub const OPUS_40: &str = "claude-opus-4-20250514";
pub const SONNET_46: &str = "claude-sonnet-4-6";
pub const SONNET_45: &str = "claude-sonnet-4-5-20250514";
pub const SONNET_40: &str = "claude-sonnet-4-20250514";
pub const SONNET_37: &str = "claude-3-7-sonnet-20250219";
pub const SONNET_35: &str = "claude-3-5-sonnet-20241022";
pub const HAIKU_45: &str = "claude-haiku-4-5-20251001";
pub const HAIKU_35: &str = "claude-3-5-haiku-20241022";

// ---------------------------------------------------------------------------
// Model aliases
// ---------------------------------------------------------------------------

/// Known short aliases that users can type instead of full model IDs.
pub const MODEL_ALIASES: &[&str] = &[
    "sonnet",
    "opus",
    "haiku",
    "best",
    "sonnet[1m]",
    "opus[1m]",
    "opusplan",
];

/// Family-level aliases used as wildcards in allowlists.
pub const MODEL_FAMILY_ALIASES: &[&str] = &["sonnet", "opus", "haiku"];

/// Returns `true` if the input is a recognised model alias.
pub fn is_model_alias(input: &str) -> bool {
    MODEL_ALIASES.contains(&input)
}

/// Returns `true` if the input is a recognised model family alias.
pub fn is_model_family_alias(input: &str) -> bool {
    MODEL_FAMILY_ALIASES.contains(&input)
}

// ---------------------------------------------------------------------------
// Canonical name mapping
// ---------------------------------------------------------------------------

/// Pure string-match that strips date/provider suffixes from a first-party
/// model name and returns a short canonical form.
///
/// E.g. `"claude-opus-4-6-20260401"` → `"claude-opus-4-6"`
pub fn first_party_name_to_canonical(name: &str) -> String {
    let name = name.to_lowercase();

    // Order matters — more specific versions first.
    let patterns: &[(&str, &str)] = &[
        ("claude-opus-4-6", "claude-opus-4-6"),
        ("claude-opus-4-5", "claude-opus-4-5"),
        ("claude-opus-4-1", "claude-opus-4-1"),
        ("claude-opus-4", "claude-opus-4"),
        ("claude-sonnet-4-6", "claude-sonnet-4-6"),
        ("claude-sonnet-4-5", "claude-sonnet-4-5"),
        ("claude-sonnet-4", "claude-sonnet-4"),
        ("claude-haiku-4-5", "claude-haiku-4-5"),
        ("claude-3-7-sonnet", "claude-3-7-sonnet"),
        ("claude-3-5-sonnet", "claude-3-5-sonnet"),
        ("claude-3-5-haiku", "claude-3-5-haiku"),
        ("claude-3-opus", "claude-3-opus"),
        ("claude-3-sonnet", "claude-3-sonnet"),
        ("claude-3-haiku", "claude-3-haiku"),
    ];

    for &(substr, canonical) in patterns {
        if name.contains(substr) {
            return canonical.to_string();
        }
    }

    // Fallback: try to match a claude-<something> pattern.
    if let Some(caps) = lazy_regex::regex!(r"(claude-(\d+-\d+-)?[\w]+)")
        .captures(&name)
    {
        if let Some(m) = caps.get(1) {
            return m.as_str().to_string();
        }
    }

    name
}

/// Maps any full model string (including 3P provider prefixes) to a canonical
/// short name.
pub fn get_canonical_name(full_model_name: &str) -> String {
    first_party_name_to_canonical(full_model_name)
}

// ---------------------------------------------------------------------------
// Model capabilities
// ---------------------------------------------------------------------------

/// Static capability matrix for a model.
#[derive(Clone, Debug)]
pub struct ModelCapabilities {
    pub context_window: u64,
    pub max_output_tokens_default: u64,
    pub max_output_tokens_upper: u64,
    pub supports_thinking: bool,
    pub supports_vision: bool,
    pub supports_tool_use: bool,
}

/// Resolve the static capability matrix for a model identified by its
/// canonical name (or full name — we canonicalise internally).
pub fn get_model_capabilities(model: &str) -> ModelCapabilities {
    let canonical = get_canonical_name(model);
    let c = canonical.as_str();

    // Defaults
    let mut caps = ModelCapabilities {
        context_window: 200_000,
        max_output_tokens_default: 32_000,
        max_output_tokens_upper: 64_000,
        supports_thinking: true,
        supports_vision: true,
        supports_tool_use: true,
    };

    if c.contains("opus-4-6") {
        caps.max_output_tokens_default = 64_000;
        caps.max_output_tokens_upper = 128_000;
    } else if c.contains("sonnet-4-6") {
        caps.max_output_tokens_default = 32_000;
        caps.max_output_tokens_upper = 128_000;
    } else if c.contains("opus-4-5") || c.contains("sonnet-4") || c.contains("haiku-4") {
        caps.max_output_tokens_default = 32_000;
        caps.max_output_tokens_upper = 64_000;
    } else if c.contains("opus-4-1") || c.contains("opus-4") {
        caps.max_output_tokens_default = 32_000;
        caps.max_output_tokens_upper = 32_000;
    } else if c.contains("claude-3-opus") {
        caps.max_output_tokens_default = 4_096;
        caps.max_output_tokens_upper = 4_096;
        caps.supports_thinking = false;
    } else if c.contains("claude-3-sonnet") {
        caps.max_output_tokens_default = 8_192;
        caps.max_output_tokens_upper = 8_192;
        caps.supports_thinking = false;
    } else if c.contains("claude-3-haiku") {
        caps.max_output_tokens_default = 4_096;
        caps.max_output_tokens_upper = 4_096;
        caps.supports_thinking = false;
    } else if c.contains("3-5-sonnet") || c.contains("3-5-haiku") {
        caps.max_output_tokens_default = 8_192;
        caps.max_output_tokens_upper = 8_192;
        caps.supports_thinking = false;
    } else if c.contains("3-7-sonnet") {
        caps.max_output_tokens_default = 32_000;
        caps.max_output_tokens_upper = 64_000;
    }

    caps
}

// ---------------------------------------------------------------------------
// Model resolution
// ---------------------------------------------------------------------------

/// Resolve a user-supplied model string (alias, full name, or with `[1m]`
/// suffix) to the concrete model ID used for API calls.
pub fn resolve_model_string(input: &str) -> String {
    let trimmed = input.trim();
    let lower = trimmed.to_lowercase();

    let has_1m = has_1m_context(&lower);
    let base = if has_1m {
        lower.replace("[1m]", "").trim().to_string()
    } else {
        lower.clone()
    };

    let suffix = if has_1m { "[1m]" } else { "" };

    let resolved = match base.as_str() {
        "opus" | "best" => default_opus_model().to_string(),
        "sonnet" | "opusplan" => default_sonnet_model().to_string(),
        "haiku" => default_haiku_model().to_string(),
        _ => {
            // Preserve original case for custom model names.
            if has_1m {
                return format!(
                    "{}[1m]",
                    trimmed
                        .trim_end_matches("[1m]")
                        .trim_end_matches("[1M]")
                        .trim()
                );
            }
            return trimmed.to_string();
        }
    };

    format!("{resolved}{suffix}")
}

/// Return the default Opus model ID, respecting the `ANTHROPIC_DEFAULT_OPUS_MODEL` env var.
pub fn default_opus_model() -> &'static str {
    // Check env at call site; static fallback otherwise.
    lazy_static_env_or(
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        OPUS_46,
    )
}

/// Return the default Sonnet model ID, respecting the `ANTHROPIC_DEFAULT_SONNET_MODEL` env var.
pub fn default_sonnet_model() -> &'static str {
    lazy_static_env_or(
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        SONNET_46,
    )
}

/// Return the default Haiku model ID, respecting the `ANTHROPIC_DEFAULT_HAIKU_MODEL` env var.
pub fn default_haiku_model() -> &'static str {
    lazy_static_env_or(
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        HAIKU_45,
    )
}

/// Helper: return the env var value if set, else the compile-time default.
/// The returned `&'static str` is leaked once per env var for lifetime convenience.
fn lazy_static_env_or(var: &str, default: &'static str) -> &'static str {
    // We intentionally leak — these are process-lifetime constants.
    match env::var(var) {
        Ok(val) if !val.is_empty() => {
            let leaked: &'static str = Box::leak(val.into_boxed_str());
            leaked
        }
        _ => default,
    }
}

/// Strip the `[1m]` / `[2m]` suffix from a model string for the API.
pub fn normalize_model_string_for_api(model: &str) -> String {
    lazy_regex::regex!(r"(?i)\[\d+m\]$")
        .replace(model, "")
        .to_string()
}

// ---------------------------------------------------------------------------
// Model validation / deprecation
// ---------------------------------------------------------------------------

/// Returns `true` if the string looks like a valid Claude model identifier.
pub fn is_valid_model(model: &str) -> bool {
    let lower = model.to_lowercase();
    // Accept known aliases.
    if is_model_alias(&lower) {
        return true;
    }
    // Accept anything that looks like a claude model string.
    lower.contains("claude")
}

/// Deprecated model entries.
struct DeprecatedEntry {
    substring: &'static str,
    model_name: &'static str,
    retirement_date: &'static str,
}

const DEPRECATED_MODELS: &[DeprecatedEntry] = &[
    DeprecatedEntry {
        substring: "claude-3-opus",
        model_name: "Claude 3 Opus",
        retirement_date: "January 5, 2026",
    },
    DeprecatedEntry {
        substring: "claude-3-7-sonnet",
        model_name: "Claude 3.7 Sonnet",
        retirement_date: "February 19, 2026",
    },
    DeprecatedEntry {
        substring: "claude-3-5-haiku",
        model_name: "Claude 3.5 Haiku",
        retirement_date: "February 19, 2026",
    },
];

/// Returns `true` if the model is deprecated.
pub fn is_deprecated_model(model: &str) -> bool {
    get_model_deprecation_warning(model).is_some()
}

/// Returns a deprecation warning string, or `None` if the model is not deprecated.
pub fn get_model_deprecation_warning(model: &str) -> Option<String> {
    let lower = model.to_lowercase();
    for entry in DEPRECATED_MODELS {
        if lower.contains(entry.substring) {
            return Some(format!(
                "{} will be retired on {}. Consider switching to a newer model.",
                entry.model_name, entry.retirement_date
            ));
        }
    }
    None
}

/// Get a human-readable display name for a model, if it's a known public model.
pub fn get_public_model_display_name(model: &str) -> Option<&'static str> {
    // Strip [1m] for matching, but include it in the display.
    let has_1m = has_1m_context(model);
    let base = normalize_model_string_for_api(model);

    match base.as_str() {
        s if s == OPUS_46 && has_1m => Some("Opus 4.6 (1M context)"),
        s if s == OPUS_46 => Some("Opus 4.6"),
        s if s == OPUS_45 => Some("Opus 4.5"),
        s if s == OPUS_41 => Some("Opus 4.1"),
        s if s == OPUS_40 => Some("Opus 4"),
        s if s == SONNET_46 && has_1m => Some("Sonnet 4.6 (1M context)"),
        s if s == SONNET_46 => Some("Sonnet 4.6"),
        s if s == SONNET_45 && has_1m => Some("Sonnet 4.5 (1M context)"),
        s if s == SONNET_45 => Some("Sonnet 4.5"),
        s if s == SONNET_40 && has_1m => Some("Sonnet 4 (1M context)"),
        s if s == SONNET_40 => Some("Sonnet 4"),
        s if s == SONNET_37 => Some("Sonnet 3.7"),
        s if s == SONNET_35 => Some("Sonnet 3.5"),
        s if s == HAIKU_45 => Some("Haiku 4.5"),
        s if s == HAIKU_35 => Some("Haiku 3.5"),
        _ => None,
    }
}

/// Returns a marketing-friendly name, e.g. "Opus 4.6 (with 1M context)".
pub fn get_marketing_name_for_model(model: &str) -> Option<&'static str> {
    let has_1m = model.to_lowercase().contains("[1m]");
    let canonical = get_canonical_name(model);
    let c = canonical.as_str();

    match () {
        _ if c.contains("claude-opus-4-6") && has_1m => Some("Opus 4.6 (with 1M context)"),
        _ if c.contains("claude-opus-4-6") => Some("Opus 4.6"),
        _ if c.contains("claude-opus-4-5") => Some("Opus 4.5"),
        _ if c.contains("claude-opus-4-1") => Some("Opus 4.1"),
        _ if c.contains("claude-opus-4") => Some("Opus 4"),
        _ if c.contains("claude-sonnet-4-6") && has_1m => Some("Sonnet 4.6 (with 1M context)"),
        _ if c.contains("claude-sonnet-4-6") => Some("Sonnet 4.6"),
        _ if c.contains("claude-sonnet-4-5") && has_1m => Some("Sonnet 4.5 (with 1M context)"),
        _ if c.contains("claude-sonnet-4-5") => Some("Sonnet 4.5"),
        _ if c.contains("claude-sonnet-4") && has_1m => Some("Sonnet 4 (with 1M context)"),
        _ if c.contains("claude-sonnet-4") => Some("Sonnet 4"),
        _ if c.contains("claude-3-7-sonnet") => Some("Claude 3.7 Sonnet"),
        _ if c.contains("claude-3-5-sonnet") => Some("Claude 3.5 Sonnet"),
        _ if c.contains("claude-haiku-4-5") => Some("Haiku 4.5"),
        _ if c.contains("claude-3-5-haiku") => Some("Claude 3.5 Haiku"),
        _ => None,
    }
}

/// Check if a model ID has the `[1m]` suffix indicating 1M context.
pub fn has_1m_context(model: &str) -> bool {
    lazy_regex::regex!(r"(?i)\[1m\]").is_match(model)
}

/// Check if a model supports 1M context (by canonical name, not suffix).
pub fn model_supports_1m(model: &str) -> bool {
    if is_1m_context_disabled() {
        return false;
    }
    let canonical = get_canonical_name(model);
    canonical.contains("claude-sonnet-4") || canonical.contains("opus-4-6")
}

/// Check if 1M context is disabled via environment variable.
pub fn is_1m_context_disabled() -> bool {
    env::var("CLAUDE_CODE_DISABLE_1M_CONTEXT")
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canonical_name() {
        assert_eq!(
            get_canonical_name("claude-opus-4-6-20260401"),
            "claude-opus-4-6"
        );
        assert_eq!(
            get_canonical_name("claude-3-5-sonnet-20241022"),
            "claude-3-5-sonnet"
        );
        assert_eq!(
            get_canonical_name("us.anthropic.claude-opus-4-6-v1:0"),
            "claude-opus-4-6"
        );
    }

    #[test]
    fn test_is_model_alias() {
        assert!(is_model_alias("opus"));
        assert!(is_model_alias("sonnet"));
        assert!(!is_model_alias("claude-opus-4-6"));
    }

    #[test]
    fn test_resolve_model_string() {
        let resolved = resolve_model_string("opus");
        assert!(resolved.contains("opus"));
        let resolved = resolve_model_string("haiku[1m]");
        assert!(resolved.contains("[1m]"));
    }

    #[test]
    fn test_has_1m_context() {
        assert!(has_1m_context("claude-opus-4-6[1m]"));
        assert!(has_1m_context("claude-opus-4-6[1M]"));
        assert!(!has_1m_context("claude-opus-4-6"));
    }

    #[test]
    fn test_normalize_for_api() {
        assert_eq!(
            normalize_model_string_for_api("claude-opus-4-6[1m]"),
            "claude-opus-4-6"
        );
        assert_eq!(
            normalize_model_string_for_api("claude-sonnet-4-6[2m]"),
            "claude-sonnet-4-6"
        );
    }

    #[test]
    fn test_deprecated_model() {
        assert!(is_deprecated_model("claude-3-opus-20240229"));
        assert!(!is_deprecated_model("claude-opus-4-6-20260401"));
    }

    #[test]
    fn test_model_capabilities() {
        let caps = get_model_capabilities("claude-opus-4-6-20260401");
        assert_eq!(caps.max_output_tokens_default, 64_000);
        assert_eq!(caps.max_output_tokens_upper, 128_000);
        assert!(caps.supports_thinking);

        let caps = get_model_capabilities("claude-3-opus-20240229");
        assert_eq!(caps.max_output_tokens_default, 4_096);
        assert!(!caps.supports_thinking);
    }
}
