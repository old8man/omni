use async_trait::async_trait;

use super::{Command, CommandContext, CommandResult};
use crate::auth::profiles;

/// Manage multiple authentication profiles.
pub struct ProfileCommand;

#[async_trait]
impl Command for ProfileCommand {
    fn name(&self) -> &str {
        "profile"
    }

    fn aliases(&self) -> &[&str] {
        &["profiles", "account", "accounts"]
    }

    fn description(&self) -> &str {
        "Manage authentication profiles (list, switch, add, remove)"
    }

    fn usage_hint(&self) -> &str {
        "[list|switch|add|remove|clean] [name]"
    }

    async fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let parts: Vec<&str> = args.split_whitespace().collect();
        let subcommand = parts.first().map(|s| *s).unwrap_or("list");

        match subcommand {
            "list" | "" => CommandResult::OpenProfileManager,
            "switch" => {
                let name = parts.get(1).map(|s| *s);
                if name.is_none() {
                    return CommandResult::OpenProfileManager;
                }
                execute_switch(name)
            }
            "add" => execute_add(),
            "remove" | "rm" | "delete" => {
                if let Some(name) = parts.get(1) {
                    execute_remove(name)
                } else {
                    CommandResult::Output(
                        "Usage: /profile remove <profile-name>\n\n\
                         Use /profile list to see available profiles."
                            .to_string(),
                    )
                }
            }
            "clean" => execute_clean(),
            _ => {
                // Treat unknown subcommand as a profile name to switch to
                execute_switch(Some(subcommand))
            }
        }
    }
}

#[allow(dead_code)]
fn execute_list() -> CommandResult {
    let all_profiles = profiles::list_profiles();
    let active_name = profiles::get_active_profile_name();

    if all_profiles.is_empty() {
        return CommandResult::Output(
            "No profiles configured.\n\n\
             Use /profile add to create a new profile via OAuth login,\n\
             or /login to authenticate."
                .to_string(),
        );
    }

    let mut output = String::from("Profiles\n========\n\n");

    for profile in &all_profiles {
        let is_active = active_name.as_deref() == Some(&profile.name);
        let marker = if is_active { "\u{25cf}" } else { " " };
        let sub = capitalize_sub(&profile.subscription_type);
        let status = profile.status_label(active_name.as_deref());
        let status_icon = match status {
            "active" => "\u{2713} active",
            "valid" => "\u{2713} valid",
            "expired" => "\u{2717} expired",
            _ => status,
        };

        output.push_str(&format!(
            "  {} {:<30} {:<10} {}\n",
            marker, profile.name, sub, status_icon,
        ));
    }

    output.push_str(&format!("\n{} profile(s) total.", all_profiles.len()));

    CommandResult::Output(output)
}

fn execute_switch(name: Option<&str>) -> CommandResult {
    match name {
        None => {
            // Open picker
            CommandResult::OpenPicker("profile".to_string())
        }
        Some(name) => {
            // Check if profile exists
            let all_profiles = profiles::list_profiles();
            let found = all_profiles.iter().find(|p| p.name == name);

            match found {
                Some(profile) => {
                    if let Err(e) = profiles::set_active_profile(name) {
                        return CommandResult::Output(format!(
                            "Failed to switch profile: {}",
                            e
                        ));
                    }
                    CommandResult::Output(format!(
                        "Switched to profile: {}",
                        profile.display_name()
                    ))
                }
                None => {
                    // Try partial match
                    let matches: Vec<_> = all_profiles
                        .iter()
                        .filter(|p| p.name.contains(name) || p.email.contains(name))
                        .collect();

                    match matches.len() {
                        0 => CommandResult::Output(format!(
                            "Profile '{}' not found.\n\nUse /profile list to see available profiles.",
                            name
                        )),
                        1 => {
                            let profile = matches[0];
                            if let Err(e) = profiles::set_active_profile(&profile.name) {
                                return CommandResult::Output(format!(
                                    "Failed to switch profile: {}",
                                    e
                                ));
                            }
                            CommandResult::Output(format!(
                                "Switched to profile: {}",
                                profile.display_name()
                            ))
                        }
                        _ => {
                            let mut msg = format!(
                                "Multiple profiles match '{}'. Be more specific:\n\n",
                                name
                            );
                            for p in &matches {
                                msg.push_str(&format!("  - {}\n", p.name));
                            }
                            CommandResult::Output(msg)
                        }
                    }
                }
            }
        }
    }
}

fn execute_add() -> CommandResult {
    CommandResult::Output(
        "To add a new profile, use /login to authenticate via OAuth.\n\
         After login, a profile will be created automatically from your account details."
            .to_string(),
    )
}

fn execute_remove(name: &str) -> CommandResult {
    let active_name = profiles::get_active_profile_name();
    let is_active = active_name.as_deref() == Some(name);

    match profiles::remove_profile(name) {
        Ok(()) => {
            let mut msg = format!("Removed profile: {}", name);
            if is_active {
                msg.push_str("\n\nThis was the active profile. No profile is now active.\nUse /profile switch to select another profile.");
            }
            CommandResult::Output(msg)
        }
        Err(e) => CommandResult::Output(format!("Failed to remove profile '{}': {}", name, e)),
    }
}

fn execute_clean() -> CommandResult {
    let removed = profiles::remove_expired_profiles();
    if removed.is_empty() {
        CommandResult::Output("No expired profiles to clean up.".to_string())
    } else {
        let mut msg = format!("Removed {} expired profile(s):\n\n", removed.len());
        for name in &removed {
            msg.push_str(&format!("  - {}\n", name));
        }
        CommandResult::Output(msg)
    }
}

#[allow(dead_code)]
fn capitalize_sub(s: &str) -> String {
    match s.to_lowercase().as_str() {
        "pro" => "Pro".to_string(),
        "max" => "Max".to_string(),
        "team" => "Team".to_string(),
        "enterprise" => "Enterprise".to_string(),
        "api" => "API".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}
