//! Types for IDE integration.
//!
//! Defines the IDE types, detection info, lockfile structures, and
//! MCP server configuration used for connecting Claude Code to IDEs
//! like VS Code, Cursor, Windsurf, and JetBrains family IDEs.

use serde::{Deserialize, Serialize};

/// Known IDE types that support Claude Code integration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IdeType {
    /// Visual Studio Code
    Vscode,
    /// Cursor
    Cursor,
    /// Windsurf
    Windsurf,
    /// PyCharm
    Pycharm,
    /// IntelliJ IDEA
    Intellij,
    /// WebStorm
    Webstorm,
    /// PhpStorm
    Phpstorm,
    /// RubyMine
    Rubymine,
    /// CLion
    Clion,
    /// GoLand
    Goland,
    /// Rider
    Rider,
    /// DataGrip
    Datagrip,
    /// AppCode
    Appcode,
    /// DataSpell
    Dataspell,
    /// Aqua
    Aqua,
    /// Gateway
    Gateway,
    /// Fleet
    Fleet,
    /// Android Studio
    Androidstudio,
}

impl IdeType {
    /// Human-readable display name.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Vscode => "VS Code",
            Self::Cursor => "Cursor",
            Self::Windsurf => "Windsurf",
            Self::Pycharm => "PyCharm",
            Self::Intellij => "IntelliJ IDEA",
            Self::Webstorm => "WebStorm",
            Self::Phpstorm => "PhpStorm",
            Self::Rubymine => "RubyMine",
            Self::Clion => "CLion",
            Self::Goland => "GoLand",
            Self::Rider => "Rider",
            Self::Datagrip => "DataGrip",
            Self::Appcode => "AppCode",
            Self::Dataspell => "DataSpell",
            Self::Aqua => "Aqua",
            Self::Gateway => "Gateway",
            Self::Fleet => "Fleet",
            Self::Androidstudio => "Android Studio",
        }
    }

    /// Whether this is a JetBrains IDE.
    pub fn is_jetbrains(&self) -> bool {
        matches!(
            self,
            Self::Pycharm
                | Self::Intellij
                | Self::Webstorm
                | Self::Phpstorm
                | Self::Rubymine
                | Self::Clion
                | Self::Goland
                | Self::Rider
                | Self::Datagrip
                | Self::Appcode
                | Self::Dataspell
                | Self::Aqua
                | Self::Gateway
                | Self::Fleet
                | Self::Androidstudio
        )
    }
}

/// Information about a detected IDE instance.
#[derive(Clone, Debug)]
pub struct DetectedIdeInfo {
    /// IDE display name.
    pub name: String,
    /// Port the IDE extension is listening on.
    pub port: u16,
    /// Workspace folders open in the IDE.
    pub workspace_folders: Vec<String>,
    /// URL to connect to (e.g. `ws://localhost:12345` or `http://localhost:12345/sse`).
    pub url: String,
    /// Whether the IDE connection has been validated.
    pub is_valid: bool,
    /// Authentication token, if required.
    pub auth_token: Option<String>,
    /// Whether the IDE is running on Windows (relevant for WSL path conversion).
    pub ide_running_in_windows: Option<bool>,
}

/// Parsed lockfile content from an IDE extension.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdeLockfileContent {
    /// Workspace folders the IDE has open.
    #[serde(default)]
    pub workspace_folders: Vec<String>,
    /// Process ID of the IDE.
    pub pid: Option<u32>,
    /// Name of the IDE.
    pub ide_name: Option<String>,
    /// Transport protocol: `"ws"` or `"sse"`.
    pub transport: Option<String>,
    /// Whether the IDE is running on Windows.
    #[serde(default)]
    pub running_in_windows: Option<bool>,
    /// Authentication token.
    pub auth_token: Option<String>,
}

/// Resolved lockfile with port extracted from filename.
#[derive(Clone, Debug)]
pub struct IdeLockfileInfo {
    /// Workspace folders the IDE has open.
    pub workspace_folders: Vec<String>,
    /// Port the IDE extension is listening on.
    pub port: u16,
    /// Process ID of the IDE.
    pub pid: Option<u32>,
    /// Name of the IDE.
    pub ide_name: Option<String>,
    /// Whether to use WebSocket transport (vs SSE).
    pub use_websocket: bool,
    /// Whether the IDE is running on Windows.
    pub running_in_windows: bool,
    /// Authentication token.
    pub auth_token: Option<String>,
}

/// IDE extension installation status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdeExtensionInstallationStatus {
    /// Extension is installed and up to date.
    Installed,
    /// Extension is being installed.
    Installing,
    /// Installation failed.
    Failed(String),
    /// Extension needs an update.
    NeedsUpdate,
}

/// MCP server configuration for an IDE connection.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IdeMcpConfig {
    /// Transport type: `"ws-ide"` or `"sse-ide"`.
    #[serde(rename = "type")]
    pub config_type: String,
    /// URL to connect to.
    pub url: String,
    /// IDE display name.
    #[serde(rename = "ideName")]
    pub ide_name: String,
    /// Authentication token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,
    /// Whether the IDE is running on Windows.
    #[serde(
        skip_serializing_if = "Option::is_none",
        rename = "ideRunningInWindows"
    )]
    pub ide_running_in_windows: Option<bool>,
    /// Scope indicator.
    pub scope: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ide_type_display_name() {
        assert_eq!(IdeType::Vscode.display_name(), "VS Code");
        assert_eq!(IdeType::Cursor.display_name(), "Cursor");
        assert_eq!(IdeType::Intellij.display_name(), "IntelliJ IDEA");
    }

    #[test]
    fn test_ide_type_is_jetbrains() {
        assert!(IdeType::Pycharm.is_jetbrains());
        assert!(IdeType::Intellij.is_jetbrains());
        assert!(!IdeType::Vscode.is_jetbrains());
        assert!(!IdeType::Cursor.is_jetbrains());
    }

    #[test]
    fn test_ide_type_serialization() {
        let json = serde_json::to_string(&IdeType::Vscode).unwrap();
        assert_eq!(json, "\"vscode\"");

        let ide: IdeType = serde_json::from_str("\"cursor\"").unwrap();
        assert_eq!(ide, IdeType::Cursor);
    }

    #[test]
    fn test_ide_lockfile_content_deserialization() {
        let json = r#"{
            "workspaceFolders": ["/home/user/project"],
            "pid": 1234,
            "ideName": "VS Code",
            "transport": "ws",
            "authToken": "secret123"
        }"#;
        let content: IdeLockfileContent = serde_json::from_str(json).unwrap();
        assert_eq!(content.workspace_folders, vec!["/home/user/project"]);
        assert_eq!(content.pid, Some(1234));
        assert_eq!(content.transport, Some("ws".to_string()));
    }

    #[test]
    fn test_ide_mcp_config_serialization() {
        let config = IdeMcpConfig {
            config_type: "ws-ide".to_string(),
            url: "ws://localhost:12345".to_string(),
            ide_name: "VS Code".to_string(),
            auth_token: Some("tok".to_string()),
            ide_running_in_windows: None,
            scope: "dynamic".to_string(),
        };
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["type"], "ws-ide");
        assert_eq!(json["ideName"], "VS Code");
        assert_eq!(json["scope"], "dynamic");
    }
}
