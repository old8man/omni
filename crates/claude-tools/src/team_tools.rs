use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

use crate::registry::{ProgressSender, ToolExecutor, ToolUseContext};
use claude_core::types::events::ToolResultData;

/// Default team configuration directory under the user's home.
fn team_config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude-omni")
        .join("teams")
}

/// Sanitize a team name for use as a directory/file name.
fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect::<String>()
        .to_lowercase()
}

// ---------------------------------------------------------------------------
// TeamCreateTool
// ---------------------------------------------------------------------------

/// Creates a new team configuration for coordinating multiple agents.
///
/// Writes a team file to `~/.claude/teams/<name>/team.json` with the lead
/// agent and team metadata.
pub struct TeamCreateTool;

#[async_trait]
impl ToolExecutor for TeamCreateTool {
    fn name(&self) -> &str {
        "TeamCreate"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "team_name": {
                    "type": "string",
                    "description": "Name for the new team to create"
                },
                "description": {
                    "type": "string",
                    "description": "Team description/purpose"
                },
                "agent_type": {
                    "type": "string",
                    "description": "Type/role of the team lead"
                }
            },
            "required": ["team_name"]
        })
    }

    async fn call(
        &self,
        input: &Value,
        ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let team_name = input["team_name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'team_name' field"))?;

        if team_name.trim().is_empty() {
            return Ok(ToolResultData {
                data: json!({ "error": "team_name is required" }),
                is_error: true,
            });
        }

        let description = input["description"].as_str().unwrap_or("");
        let agent_type = input["agent_type"].as_str().unwrap_or("team-lead");

        let safe_name = sanitize_name(team_name);
        let team_dir = team_config_dir().join(&safe_name);

        // Check if already exists
        if team_dir.exists() {
            return Ok(ToolResultData {
                data: json!({
                    "error": format!("Team '{}' already exists", team_name)
                }),
                is_error: true,
            });
        }

        // Create directories
        tokio::fs::create_dir_all(&team_dir).await.map_err(|e| {
            anyhow::anyhow!("Failed to create team directory: {}", e)
        })?;

        let lead_agent_id = format!("team-lead@{}", safe_name);
        let team_file_path = team_dir.join("team.json");

        let team_file = json!({
            "name": team_name,
            "description": description,
            "createdAt": chrono::Utc::now().to_rfc3339(),
            "leadAgentId": lead_agent_id,
            "cwd": ctx.working_directory.to_string_lossy(),
            "members": [
                {
                    "agentId": lead_agent_id,
                    "name": "team-lead",
                    "agentType": agent_type,
                    "joinedAt": chrono::Utc::now().to_rfc3339(),
                    "cwd": ctx.working_directory.to_string_lossy(),
                }
            ]
        });

        let content = serde_json::to_string_pretty(&team_file)?;
        tokio::fs::write(&team_file_path, &content).await.map_err(|e| {
            anyhow::anyhow!("Failed to write team file: {}", e)
        })?;

        // Create mailbox and tasks directories
        let _ = tokio::fs::create_dir_all(team_dir.join("mailbox")).await;
        let _ = tokio::fs::create_dir_all(team_dir.join("tasks")).await;

        Ok(ToolResultData {
            data: json!({
                "team_name": team_name,
                "team_file_path": team_file_path.to_string_lossy(),
                "lead_agent_id": lead_agent_id,
            }),
            is_error: false,
        })
    }
}

// ---------------------------------------------------------------------------
// TeamDeleteTool
// ---------------------------------------------------------------------------

/// Cleans up a team's configuration files and directories.
pub struct TeamDeleteTool;

#[async_trait]
impl ToolExecutor for TeamDeleteTool {
    fn name(&self) -> &str {
        "TeamDelete"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn call(
        &self,
        _input: &Value,
        ctx: &ToolUseContext,
        _cancel: CancellationToken,
        _progress: Option<ProgressSender>,
    ) -> Result<ToolResultData> {
        let team_name = match ctx.team_name.as_ref() {
            Some(name) => name.clone(),
            None => {
                return Ok(ToolResultData {
                    data: json!({
                        "success": true,
                        "message": "No team name found, nothing to clean up",
                    }),
                    is_error: false,
                });
            }
        };

        let safe_name = sanitize_name(&team_name);
        let team_dir = team_config_dir().join(&safe_name);

        if team_dir.exists() {
            // Check for active members before deleting
            let team_file_path = team_dir.join("team.json");
            if team_file_path.exists() {
                if let Ok(content) = tokio::fs::read_to_string(&team_file_path).await {
                    if let Ok(team_file) = serde_json::from_str::<Value>(&content) {
                        if let Some(members) = team_file["members"].as_array() {
                            let non_lead: Vec<&Value> = members
                                .iter()
                                .filter(|m| m["name"].as_str() != Some("team-lead"))
                                .collect();

                            let active: Vec<&&Value> = non_lead
                                .iter()
                                .filter(|m| m["isActive"].as_bool() != Some(false))
                                .collect();

                            if !active.is_empty() {
                                let names: Vec<&str> = active
                                    .iter()
                                    .filter_map(|m| m["name"].as_str())
                                    .collect();
                                return Ok(ToolResultData {
                                    data: json!({
                                        "success": false,
                                        "message": format!(
                                            "Cannot cleanup team with {} active member(s): {}. Use requestShutdown to gracefully terminate teammates first.",
                                            active.len(),
                                            names.join(", ")
                                        ),
                                        "team_name": team_name,
                                    }),
                                    is_error: false,
                                });
                            }
                        }
                    }
                }
            }

            if let Err(e) = tokio::fs::remove_dir_all(&team_dir).await {
                return Ok(ToolResultData {
                    data: json!({
                        "success": false,
                        "message": format!("Failed to remove team directory: {}", e),
                        "team_name": team_name,
                    }),
                    is_error: true,
                });
            }
        }

        Ok(ToolResultData {
            data: json!({
                "success": true,
                "message": format!("Cleaned up directories for team \"{}\"", team_name),
                "team_name": team_name,
            }),
            is_error: false,
        })
    }
}
