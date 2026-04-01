//! Multi-profile authentication system.
//!
//! Profiles are stored under `~/.claude-omni/profiles/` with each profile
//! in its own directory named `{email}-{subscriptionType}`. Each directory
//! contains a `credentials.json` file with the stored tokens and metadata.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A user profile containing authentication credentials and metadata.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    /// Profile name in the format "email-subscriptionType" (e.g. "user@gmail.com-pro").
    pub name: String,
    /// The user's email address.
    pub email: String,
    /// Subscription type: "pro", "max", "team", "enterprise", or "api".
    pub subscription_type: String,
    /// Authentication credentials for this profile.
    pub credentials: ProfileCredentials,
    /// ISO 8601 timestamp when this profile was created.
    pub created_at: String,
}

/// Stored credentials for a single profile.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProfileCredentials {
    /// OAuth access token (for Claude.ai subscribers).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    /// OAuth refresh token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Token expiration time in milliseconds since epoch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    /// Direct API key (for Console API key users).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// OAuth scopes granted to this token.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Account UUID from the OAuth provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_uuid: Option<String>,
    /// Organization name associated with the account.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization_name: Option<String>,
}

impl Profile {
    /// Returns true if the access token has expired (with a 5-minute buffer).
    pub fn is_expired(&self) -> bool {
        match self.credentials.expires_at {
            Some(expires_at) => {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                // 5-minute buffer
                now_ms + 300_000 >= expires_at
            }
            None => {
                // No expiration set; if there's an api_key it's valid, otherwise treat as expired
                self.credentials.api_key.is_none() && self.credentials.access_token.is_some()
            }
        }
    }

    /// Human-readable display name, e.g. "user@gmail.com (Pro)".
    pub fn display_name(&self) -> String {
        let sub = capitalize_first(&self.subscription_type);
        format!("{} ({})", self.email, sub)
    }

    /// Returns a status string: "active", "valid", or "expired".
    pub fn status_label(&self, active_name: Option<&str>) -> &'static str {
        if active_name == Some(self.name.as_str()) {
            "active"
        } else if self.is_expired() {
            "expired"
        } else {
            "valid"
        }
    }
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

/// Returns the path to `~/.claude-omni/profiles/`.
fn profiles_dir() -> Result<PathBuf> {
    let dir = crate::config::paths::claude_dir()?;
    Ok(dir.join("profiles"))
}

/// Returns the path to `~/.claude-omni/active_profile`.
fn active_profile_path() -> Result<PathBuf> {
    let dir = crate::config::paths::claude_dir()?;
    Ok(dir.join("active_profile"))
}

/// Construct a profile name from an email and subscription type.
pub fn profile_name_from_email(email: &str, sub_type: &str) -> String {
    format!("{}-{}", email, sub_type.to_lowercase())
}

/// List all profiles by scanning the profiles directory.
pub fn list_profiles() -> Vec<Profile> {
    let dir = match profiles_dir() {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };

    if !dir.exists() {
        return Vec::new();
    }

    let mut profiles = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let creds_path = path.join("credentials.json");
        if !creds_path.exists() {
            continue;
        }
        match std::fs::read_to_string(&creds_path) {
            Ok(content) => {
                if let Ok(profile) = serde_json::from_str::<Profile>(&content) {
                    profiles.push(profile);
                }
            }
            Err(_) => continue,
        }
    }

    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    profiles
}

/// Get the currently active profile, or None if no active profile is set.
pub fn get_active_profile() -> Option<Profile> {
    let path = active_profile_path().ok()?;
    if !path.exists() {
        return None;
    }
    let name = std::fs::read_to_string(&path).ok()?.trim().to_string();
    if name.is_empty() {
        return None;
    }
    load_profile(&name)
}

/// Get the name of the active profile without loading it.
pub fn get_active_profile_name() -> Option<String> {
    let path = active_profile_path().ok()?;
    if !path.exists() {
        return None;
    }
    let name = std::fs::read_to_string(&path).ok()?.trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// Load a specific profile by name.
fn load_profile(name: &str) -> Option<Profile> {
    let dir = profiles_dir().ok()?;
    let creds_path = dir.join(name).join("credentials.json");
    if !creds_path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&creds_path).ok()?;
    serde_json::from_str::<Profile>(&content).ok()
}

/// Set the active profile by writing the profile name to the active_profile file.
pub fn set_active_profile(name: &str) -> Result<()> {
    let path = active_profile_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, name)?;
    Ok(())
}

/// Save a profile's credentials to disk.
pub fn save_profile(profile: &Profile) -> Result<()> {
    let dir = profiles_dir()?;
    let profile_dir = dir.join(&profile.name);
    std::fs::create_dir_all(&profile_dir)
        .with_context(|| format!("create profile dir: {}", profile_dir.display()))?;

    let creds_path = profile_dir.join("credentials.json");
    let json = serde_json::to_string_pretty(profile)
        .context("serialize profile credentials")?;
    std::fs::write(&creds_path, json)?;

    // Set restrictive file permissions (owner read/write only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&creds_path, perms).ok();
    }

    Ok(())
}

/// Remove a profile by deleting its directory.
pub fn remove_profile(name: &str) -> Result<()> {
    let dir = profiles_dir()?;
    let profile_dir = dir.join(name);
    if profile_dir.exists() {
        std::fs::remove_dir_all(&profile_dir)
            .with_context(|| format!("remove profile dir: {}", profile_dir.display()))?;
    }

    // If this was the active profile, clear the active_profile file
    if let Some(active) = get_active_profile_name() {
        if active == name {
            let active_path = active_profile_path()?;
            std::fs::write(&active_path, "")?;
        }
    }

    Ok(())
}

/// Remove all expired profiles that have no refresh token.
pub fn remove_expired_profiles() -> Vec<String> {
    let profiles = list_profiles();
    let mut removed = Vec::new();

    for profile in &profiles {
        if profile.is_expired() && profile.credentials.refresh_token.is_none() {
            if let Ok(()) = remove_profile(&profile.name) {
                removed.push(profile.name.clone());
            }
        }
    }

    if !removed.is_empty() {
        tracing::info!("Removed {} expired profile(s): {:?}", removed.len(), removed);
    }

    removed
}

/// Convenience function: get the active profile's credentials for the API client.
/// Returns the credentials from the active profile, or None if no active profile.
pub fn get_active_credentials() -> Option<ProfileCredentials> {
    get_active_profile().map(|p| p.credentials)
}

/// Convert stored OAuth tokens to a Profile and save it.
pub fn save_oauth_as_profile(
    tokens: &super::storage::OAuthStoredTokens,
    email: &str,
    subscription_type: &str,
) -> Result<Profile> {
    let name = profile_name_from_email(email, subscription_type);
    let now = chrono::Utc::now().to_rfc3339();

    let profile = Profile {
        name: name.clone(),
        email: email.to_string(),
        subscription_type: subscription_type.to_lowercase(),
        credentials: ProfileCredentials {
            access_token: Some(tokens.access_token.clone()),
            refresh_token: tokens.refresh_token.clone(),
            expires_at: tokens.expires_at,
            api_key: None,
            scopes: tokens.scopes.clone(),
            account_uuid: None,
            organization_name: None,
        },
        created_at: now,
    };

    save_profile(&profile)?;
    set_active_profile(&name)?;

    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profile_name_from_email() {
        assert_eq!(
            profile_name_from_email("user@gmail.com", "Pro"),
            "user@gmail.com-pro"
        );
        assert_eq!(
            profile_name_from_email("work@corp.com", "MAX"),
            "work@corp.com-max"
        );
    }

    #[test]
    fn test_capitalize_first() {
        assert_eq!(capitalize_first("pro"), "Pro");
        assert_eq!(capitalize_first("max"), "Max");
        assert_eq!(capitalize_first(""), "");
        assert_eq!(capitalize_first("API"), "API");
    }

    #[test]
    fn test_profile_display_name() {
        let profile = Profile {
            name: "user@gmail.com-pro".to_string(),
            email: "user@gmail.com".to_string(),
            subscription_type: "pro".to_string(),
            credentials: ProfileCredentials {
                access_token: None,
                refresh_token: None,
                expires_at: None,
                api_key: None,
                scopes: vec![],
                account_uuid: None,
                organization_name: None,
            },
            created_at: "2025-01-01T00:00:00Z".to_string(),
        };
        assert_eq!(profile.display_name(), "user@gmail.com (Pro)");
    }

    #[test]
    fn test_is_expired_no_expiry_no_api_key() {
        let profile = Profile {
            name: "test".to_string(),
            email: "test@test.com".to_string(),
            subscription_type: "pro".to_string(),
            credentials: ProfileCredentials {
                access_token: Some("token".to_string()),
                refresh_token: None,
                expires_at: None,
                api_key: None,
                scopes: vec![],
                account_uuid: None,
                organization_name: None,
            },
            created_at: "2025-01-01T00:00:00Z".to_string(),
        };
        assert!(profile.is_expired());
    }

    #[test]
    fn test_is_expired_with_api_key() {
        let profile = Profile {
            name: "test".to_string(),
            email: "test@test.com".to_string(),
            subscription_type: "api".to_string(),
            credentials: ProfileCredentials {
                access_token: None,
                refresh_token: None,
                expires_at: None,
                api_key: Some("sk-ant-...".to_string()),
                scopes: vec![],
                account_uuid: None,
                organization_name: None,
            },
            created_at: "2025-01-01T00:00:00Z".to_string(),
        };
        assert!(!profile.is_expired());
    }

    #[test]
    fn test_is_expired_future_expiry() {
        let future_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            + 3_600_000; // 1 hour from now
        let profile = Profile {
            name: "test".to_string(),
            email: "test@test.com".to_string(),
            subscription_type: "pro".to_string(),
            credentials: ProfileCredentials {
                access_token: Some("token".to_string()),
                refresh_token: None,
                expires_at: Some(future_ms),
                api_key: None,
                scopes: vec![],
                account_uuid: None,
                organization_name: None,
            },
            created_at: "2025-01-01T00:00:00Z".to_string(),
        };
        assert!(!profile.is_expired());
    }

    #[test]
    fn test_status_label() {
        let profile = Profile {
            name: "user@gmail.com-pro".to_string(),
            email: "user@gmail.com".to_string(),
            subscription_type: "pro".to_string(),
            credentials: ProfileCredentials {
                access_token: None,
                refresh_token: None,
                expires_at: None,
                api_key: Some("key".to_string()),
                scopes: vec![],
                account_uuid: None,
                organization_name: None,
            },
            created_at: "2025-01-01T00:00:00Z".to_string(),
        };
        assert_eq!(
            profile.status_label(Some("user@gmail.com-pro")),
            "active"
        );
        assert_eq!(profile.status_label(None), "valid");
        assert_eq!(
            profile.status_label(Some("other-profile")),
            "valid"
        );
    }
}
