use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// Record of which migrations have been applied.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct MigrationState {
    /// Names of migrations that have already been applied.
    #[serde(default)]
    completed: HashSet<String>,
}

/// A migration that can be applied to update settings or config.
struct Migration {
    /// Unique name for this migration.
    name: &'static str,
    /// The migration function.
    migrate: fn(&Path) -> Result<()>,
}

/// Run all pending migrations against the config directory.
///
/// Migrations are idempotent and tracked in `<config_dir>/migrations.json`.
/// Each migration is run at most once.
pub fn run_pending_migrations(config_dir: &Path) -> Result<()> {
    let state_path = config_dir.join("migrations.json");
    let mut state = load_migration_state(&state_path);

    let migrations = all_migrations();
    let mut applied = 0;

    for migration in &migrations {
        if state.completed.contains(migration.name) {
            continue;
        }

        debug!(migration = migration.name, "running migration");

        match (migration.migrate)(config_dir) {
            Ok(()) => {
                state.completed.insert(migration.name.to_string());
                applied += 1;
                debug!(migration = migration.name, "migration completed");
            }
            Err(e) => {
                warn!(migration = migration.name, "migration failed: {e:#}");
                // Continue with other migrations; don't mark as completed
            }
        }
    }

    if applied > 0 {
        save_migration_state(&state_path, &state)?;
        info!(applied, "migrations applied");
    }

    Ok(())
}

/// Load the migration state file, returning defaults if it doesn't exist.
fn load_migration_state(path: &Path) -> MigrationState {
    match std::fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => MigrationState::default(),
    }
}

/// Save the migration state file.
fn save_migration_state(path: &Path, state: &MigrationState) -> Result<()> {
    let json = serde_json::to_string_pretty(state)?;
    std::fs::write(path, json).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// All known migrations in application order.
fn all_migrations() -> Vec<Migration> {
    vec![
        Migration {
            name: "migrate_fennec_to_opus",
            migrate: migrate_fennec_to_opus,
        },
        Migration {
            name: "migrate_opus_to_opus1m",
            migrate: migrate_opus_to_opus1m,
        },
        Migration {
            name: "migrate_sonnet1m_to_sonnet45",
            migrate: migrate_sonnet1m_to_sonnet45,
        },
        Migration {
            name: "migrate_sonnet45_to_sonnet46",
            migrate: migrate_sonnet45_to_sonnet46,
        },
        Migration {
            name: "migrate_auto_updates_to_settings",
            migrate: migrate_auto_updates_to_settings,
        },
    ]
}

// ── Individual migration functions ──────────────────────────────────────────

/// Migrate removed fennec model aliases to their Opus 4.6 equivalents.
///
/// - fennec-latest -> opus
/// - fennec-latest[1m] -> opus[1m]
/// - fennec-fast-latest -> opus[1m]
/// - opus-4-5-fast -> opus[1m]
fn migrate_fennec_to_opus(config_dir: &Path) -> Result<()> {
    let settings_path = config_dir.join("settings.json");
    let mut settings = load_settings_json(&settings_path)?;

    let model = settings
        .get("model")
        .and_then(|v| v.as_str())
        .map(String::from);

    if let Some(model) = model {
        let new_model = if model.starts_with("fennec-latest[1m]") {
            Some("opus[1m]")
        } else if model.starts_with("fennec-latest") {
            Some("opus")
        } else if model.starts_with("fennec-fast-latest") || model.starts_with("opus-4-5-fast") {
            Some("opus[1m]")
        } else {
            None
        };

        if let Some(new) = new_model {
            settings["model"] = serde_json::Value::String(new.to_string());
            save_settings_json(&settings_path, &settings)?;
            info!(from = %model, to = new, "migrated model setting");
        }
    }

    Ok(())
}

/// Migrate users with 'opus' pinned to 'opus[1m]' for the merged Opus 1M experience.
fn migrate_opus_to_opus1m(config_dir: &Path) -> Result<()> {
    let settings_path = config_dir.join("settings.json");
    let mut settings = load_settings_json(&settings_path)?;

    if let Some(model) = settings.get("model").and_then(|v| v.as_str()) {
        if model == "opus" {
            settings["model"] = serde_json::Value::String("opus[1m]".to_string());
            save_settings_json(&settings_path, &settings)?;
            info!("migrated opus -> opus[1m]");
        }
    }

    Ok(())
}

/// Migrate sonnet[1m] to the explicit sonnet-4-5-20250929[1m] to preserve
/// the intended model now that the 'sonnet' alias resolves to Sonnet 4.6.
fn migrate_sonnet1m_to_sonnet45(config_dir: &Path) -> Result<()> {
    let settings_path = config_dir.join("settings.json");
    let mut settings = load_settings_json(&settings_path)?;

    if let Some(model) = settings.get("model").and_then(|v| v.as_str()) {
        if model == "sonnet[1m]" {
            settings["model"] = serde_json::Value::String("sonnet-4-5-20250929[1m]".to_string());
            save_settings_json(&settings_path, &settings)?;
            info!("migrated sonnet[1m] -> sonnet-4-5-20250929[1m]");
        }
    }

    Ok(())
}

/// Migrate explicit Sonnet 4.5 model strings to the 'sonnet' alias
/// (which now resolves to Sonnet 4.6).
fn migrate_sonnet45_to_sonnet46(config_dir: &Path) -> Result<()> {
    let settings_path = config_dir.join("settings.json");
    let mut settings = load_settings_json(&settings_path)?;

    let sonnet45_models = [
        "claude-sonnet-4-5-20250929",
        "claude-sonnet-4-5-20250929[1m]",
        "sonnet-4-5-20250929",
        "sonnet-4-5-20250929[1m]",
    ];

    let model = settings
        .get("model")
        .and_then(|v| v.as_str())
        .map(String::from);

    if let Some(model) = model {
        if sonnet45_models.contains(&model.as_str()) {
            let has_1m = model.ends_with("[1m]");
            let new_model = if has_1m { "sonnet[1m]" } else { "sonnet" };
            settings["model"] = serde_json::Value::String(new_model.to_string());
            save_settings_json(&settings_path, &settings)?;
            info!(from = %model, to = new_model, "migrated sonnet 4.5 -> 4.6 alias");
        }
    }

    Ok(())
}

/// Migrate auto-update settings from config.json to settings.json.
fn migrate_auto_updates_to_settings(config_dir: &Path) -> Result<()> {
    let config_path = config_dir.join("config.json");
    let settings_path = config_dir.join("settings.json");

    let config = load_settings_json(&config_path)?;

    // Check if auto-updates config exists in old location
    if let Some(auto_update) = config.get("autoUpdates") {
        let mut settings = load_settings_json(&settings_path)?;

        // Only migrate if not already present in settings
        if settings.get("autoUpdater").is_none() {
            settings["autoUpdater"] = auto_update.clone();
            save_settings_json(&settings_path, &settings)?;
            info!("migrated auto-update settings to settings.json");
        }
    }

    Ok(())
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Load a JSON file as a Value, returning an empty object if not found.
fn load_settings_json(path: &Path) -> Result<serde_json::Value> {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let value: serde_json::Value = serde_json::from_str(&content)
                .with_context(|| format!("parsing {}", path.display()))?;
            Ok(value)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::json!({})),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Save a JSON Value to a file, creating parent directories as needed.
fn save_settings_json(path: &Path, value: &serde_json::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(value)?;
    std::fs::write(path, json).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_migration_state_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("migrations.json");

        let mut state = MigrationState::default();
        state.completed.insert("test_migration".to_string());
        save_migration_state(&path, &state).unwrap();

        let loaded = load_migration_state(&path);
        assert!(loaded.completed.contains("test_migration"));
    }

    #[test]
    fn test_migration_state_missing_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.json");
        let state = load_migration_state(&path);
        assert!(state.completed.is_empty());
    }

    #[test]
    fn test_fennec_migration() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        std::fs::write(&settings_path, r#"{"model": "fennec-latest"}"#).unwrap();

        migrate_fennec_to_opus(dir.path()).unwrap();

        let settings: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert_eq!(settings["model"].as_str().unwrap(), "opus");
    }

    #[test]
    fn test_run_pending_migrations_idempotent() {
        let dir = TempDir::new().unwrap();
        // Create a minimal settings.json
        std::fs::write(dir.path().join("settings.json"), "{}").unwrap();

        run_pending_migrations(dir.path()).unwrap();
        // Running again should be a no-op
        run_pending_migrations(dir.path()).unwrap();

        let state = load_migration_state(&dir.path().join("migrations.json"));
        assert_eq!(state.completed.len(), all_migrations().len());
    }
}
