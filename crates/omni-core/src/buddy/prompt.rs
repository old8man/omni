//! System prompt generation for the companion personality.
//!
//! The companion is a small creature that sits beside the user's input box and
//! occasionally comments in a speech bubble. The model is instructed to stay
//! out of the way when the user addresses the companion directly.

/// Generate the companion introduction text for the system prompt.
///
/// This text tells the model about the companion so it can respond
/// appropriately when the user talks to or about their companion.
pub fn companion_intro_text(name: &str, species: &str) -> String {
    format!(
        r#"# Companion

A small {species} named {name} sits beside the user's input box and occasionally comments in a speech bubble. You're not {name} -- it's a separate watcher.

When the user addresses {name} directly (by name), its bubble will answer. Your job in that moment is to stay out of the way: respond in ONE line or less, or just answer any part of the message meant for you. Don't explain that you're not {name} -- they know. Don't narrate what {name} might say -- the bubble handles that."#
    )
}

/// Attachment type for companion introduction in the message stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanionIntroAttachment {
    pub name: String,
    pub species: String,
}

/// Check whether a companion intro attachment has already been sent for
/// this companion name. Returns the intro attachment if it should be sent,
/// or `None` if it was already announced.
///
/// The `existing_names` parameter should contain the companion names from
/// any prior `companion_intro` attachments in the message history.
pub fn get_companion_intro_attachment(
    companion_name: &str,
    companion_species: &str,
    existing_names: &[&str],
) -> Option<CompanionIntroAttachment> {
    if existing_names.contains(&companion_name) {
        return None;
    }

    Some(CompanionIntroAttachment {
        name: companion_name.to_string(),
        species: companion_species.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intro_text_contains_name_and_species() {
        let text = companion_intro_text("Quackers", "duck");
        assert!(text.contains("Quackers"));
        assert!(text.contains("duck"));
        assert!(text.contains("# Companion"));
    }

    #[test]
    fn intro_attachment_not_sent_twice() {
        let att = get_companion_intro_attachment("Quackers", "duck", &[]);
        assert!(att.is_some());

        let att = get_companion_intro_attachment("Quackers", "duck", &["Quackers"]);
        assert!(att.is_none());
    }

    #[test]
    fn intro_attachment_sent_for_different_companion() {
        let att =
            get_companion_intro_attachment("Blobby", "blob", &["Quackers"]);
        assert!(att.is_some());
        assert_eq!(att.unwrap().name, "Blobby");
    }
}
