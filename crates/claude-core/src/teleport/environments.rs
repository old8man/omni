//! Environment Detection and Selection
//!
//! Detects available remote environments, provides environment selection logic,
//! and manages environment configuration for session migration.

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::api::CCR_BYOC_BETA;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Kind of environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentKind {
    AnthropicCloud,
    Byoc,
    Bridge,
}

/// State of an environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentState {
    Active,
}

/// An environment resource returned by the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentResource {
    pub kind: EnvironmentKind,
    pub environment_id: String,
    pub name: String,
    pub created_at: String,
    pub state: EnvironmentState,
}

/// Paginated list of environments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentListResponse {
    pub environments: Vec<EnvironmentResource>,
    pub has_more: bool,
    pub first_id: Option<String>,
    pub last_id: Option<String>,
}

/// Result of environment selection, including the source of the selection.
#[derive(Debug, Clone)]
pub struct EnvironmentSelectionInfo {
    /// All environments available from the API.
    pub available_environments: Vec<EnvironmentResource>,
    /// The environment that would be used (based on settings or first available).
    pub selected_environment: Option<EnvironmentResource>,
    /// The source of the selection (e.g. "project", "user", "enterprise"), or
    /// `None` if using the default (first non-bridge environment).
    pub selected_environment_source: Option<String>,
}

/// Configuration for creating a default cloud environment.
#[derive(Debug, Clone, Serialize)]
struct CreateCloudEnvironmentRequest {
    name: String,
    kind: String,
    description: String,
    config: CloudEnvironmentConfig,
}

#[derive(Debug, Clone, Serialize)]
struct CloudEnvironmentConfig {
    environment_type: String,
    cwd: String,
    init_script: Option<String>,
    environment: HashMap<String, String>,
    languages: Vec<LanguageSpec>,
    network_config: NetworkConfig,
}

#[derive(Debug, Clone, Serialize)]
struct LanguageSpec {
    name: String,
    version: String,
}

#[derive(Debug, Clone, Serialize)]
struct NetworkConfig {
    allowed_hosts: Vec<String>,
    allow_default_hosts: bool,
}

// ---------------------------------------------------------------------------
// API client functions
// ---------------------------------------------------------------------------

/// Fetch the list of available environments from the Environment API.
pub async fn fetch_environments(
    base_url: &str,
    access_token: &str,
    org_uuid: &str,
) -> Result<Vec<EnvironmentResource>> {
    let client = Client::new();
    let url = format!("{base_url}/v1/environment_providers");

    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Content-Type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .header("x-organization-uuid", org_uuid)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .context("Failed to fetch environments")?;

    if !resp.status().is_success() {
        bail!(
            "Failed to fetch environments: {} {}",
            resp.status().as_u16(),
            resp.status().canonical_reason().unwrap_or("Unknown")
        );
    }

    let list: EnvironmentListResponse = resp
        .json()
        .await
        .context("Failed to parse environments response")?;

    Ok(list.environments)
}

/// Create a default `anthropic_cloud` environment for users who have none.
pub async fn create_default_cloud_environment(
    base_url: &str,
    access_token: &str,
    org_uuid: &str,
    name: &str,
) -> Result<EnvironmentResource> {
    let client = Client::new();
    let url = format!("{base_url}/v1/environment_providers/cloud/create");

    let body = CreateCloudEnvironmentRequest {
        name: name.to_string(),
        kind: "anthropic_cloud".into(),
        description: String::new(),
        config: CloudEnvironmentConfig {
            environment_type: "anthropic".into(),
            cwd: "/home/user".into(),
            init_script: None,
            environment: HashMap::new(),
            languages: vec![
                LanguageSpec {
                    name: "python".into(),
                    version: "3.11".into(),
                },
                LanguageSpec {
                    name: "node".into(),
                    version: "20".into(),
                },
            ],
            network_config: NetworkConfig {
                allowed_hosts: vec![],
                allow_default_hosts: true,
            },
        },
    };

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Content-Type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", CCR_BYOC_BETA)
        .header("x-organization-uuid", org_uuid)
        .timeout(std::time::Duration::from_secs(15))
        .json(&body)
        .send()
        .await
        .context("Failed to create cloud environment")?;

    if !resp.status().is_success() {
        bail!(
            "Failed to create cloud environment: {} {}",
            resp.status().as_u16(),
            resp.status().canonical_reason().unwrap_or("Unknown")
        );
    }

    resp.json()
        .await
        .context("Failed to parse environment creation response")
}

/// Select the best environment from a list, optionally using a preferred environment ID.
///
/// Selection logic:
/// 1. If `preferred_id` is provided and matches an available environment, use it.
/// 2. Otherwise, use the first non-bridge environment.
/// 3. Fall back to the first environment of any kind.
pub fn select_environment(
    environments: &[EnvironmentResource],
    preferred_id: Option<&str>,
) -> EnvironmentSelectionInfo {
    if environments.is_empty() {
        return EnvironmentSelectionInfo {
            available_environments: vec![],
            selected_environment: None,
            selected_environment_source: None,
        };
    }

    // Check for preferred environment ID
    if let Some(pref_id) = preferred_id {
        if let Some(env) = environments
            .iter()
            .find(|e| e.environment_id == pref_id)
        {
            return EnvironmentSelectionInfo {
                available_environments: environments.to_vec(),
                selected_environment: Some(env.clone()),
                selected_environment_source: Some("settings".into()),
            };
        }
    }

    // Default: first non-bridge environment, or first of any kind
    let selected = environments
        .iter()
        .find(|e| e.kind != EnvironmentKind::Bridge)
        .or_else(|| environments.first())
        .cloned();

    EnvironmentSelectionInfo {
        available_environments: environments.to_vec(),
        selected_environment: selected,
        selected_environment_source: None,
    }
}

/// Detect whether the current runtime is inside a remote environment
/// (e.g. CCR container, Codespace, SSH session).
pub fn detect_remote_environment() -> Option<String> {
    // CCR container detection
    if std::env::var("CCR_SESSION_ID").is_ok() {
        return Some("ccr".into());
    }

    // GitHub Codespace detection
    if std::env::var("CODESPACES").is_ok() {
        return Some("codespace".into());
    }

    // Generic SSH session detection
    if std::env::var("SSH_CONNECTION").is_ok() || std::env::var("SSH_CLIENT").is_ok() {
        return Some("ssh".into());
    }

    // Docker/container detection
    if std::path::Path::new("/.dockerenv").exists() {
        return Some("docker".into());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_env(id: &str, kind: EnvironmentKind) -> EnvironmentResource {
        EnvironmentResource {
            kind,
            environment_id: id.into(),
            name: format!("env-{id}"),
            created_at: "2025-01-01".into(),
            state: EnvironmentState::Active,
        }
    }

    #[test]
    fn select_preferred_environment() {
        let envs = vec![
            make_env("a", EnvironmentKind::AnthropicCloud),
            make_env("b", EnvironmentKind::Byoc),
        ];
        let info = select_environment(&envs, Some("b"));
        assert_eq!(
            info.selected_environment.unwrap().environment_id,
            "b"
        );
        assert_eq!(
            info.selected_environment_source.as_deref(),
            Some("settings")
        );
    }

    #[test]
    fn select_default_skips_bridge() {
        let envs = vec![
            make_env("bridge-1", EnvironmentKind::Bridge),
            make_env("cloud-1", EnvironmentKind::AnthropicCloud),
        ];
        let info = select_environment(&envs, None);
        assert_eq!(
            info.selected_environment.unwrap().environment_id,
            "cloud-1"
        );
        assert!(info.selected_environment_source.is_none());
    }

    #[test]
    fn select_fallback_to_first() {
        let envs = vec![make_env("bridge-only", EnvironmentKind::Bridge)];
        let info = select_environment(&envs, None);
        assert_eq!(
            info.selected_environment.unwrap().environment_id,
            "bridge-only"
        );
    }

    #[test]
    fn select_empty_environments() {
        let info = select_environment(&[], None);
        assert!(info.selected_environment.is_none());
        assert!(info.selected_environment_source.is_none());
    }

    #[test]
    fn select_preferred_not_found_falls_back() {
        let envs = vec![make_env("a", EnvironmentKind::AnthropicCloud)];
        let info = select_environment(&envs, Some("nonexistent"));
        assert_eq!(
            info.selected_environment.unwrap().environment_id,
            "a"
        );
        assert!(info.selected_environment_source.is_none());
    }
}
