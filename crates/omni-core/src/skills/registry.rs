//! Skill registry for name-based and trigger-based lookups.

use std::collections::HashMap;

use super::types::Skill;

/// Registry holding all discovered skills with fast lookups.
#[derive(Debug, Clone, Default)]
pub struct SkillRegistry {
    skills: HashMap<String, Skill>,
    /// Insertion-order keys for deterministic listing.
    ordered: Vec<String>,
}

impl SkillRegistry {
    /// Build a registry from a list of skills.
    pub fn from_skills(skills: Vec<Skill>) -> Self {
        let mut reg = Self::default();
        for skill in skills {
            reg.register(skill);
        }
        reg
    }

    /// Register a single skill. Duplicates (by name) are silently ignored.
    pub fn register(&mut self, skill: Skill) {
        if self.skills.contains_key(&skill.name) {
            return;
        }
        self.ordered.push(skill.name.clone());
        self.skills.insert(skill.name.clone(), skill);
    }

    /// Look up a skill by exact name.
    pub fn find_by_name(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    /// Find skills whose name or description contains the given substring (case-insensitive).
    pub fn find_by_trigger(&self, trigger: &str) -> Vec<&Skill> {
        let lower = trigger.to_lowercase();
        self.ordered
            .iter()
            .filter_map(|name| self.skills.get(name))
            .filter(|s| {
                s.name.to_lowercase().contains(&lower)
                    || s.description.to_lowercase().contains(&lower)
            })
            .collect()
    }

    /// List all skills in registration order.
    pub fn list_all(&self) -> Vec<&Skill> {
        self.ordered
            .iter()
            .filter_map(|name| self.skills.get(name))
            .collect()
    }

    /// List only user-invocable skills.
    pub fn list_user_invocable(&self) -> Vec<&Skill> {
        self.list_all()
            .into_iter()
            .filter(|s| s.user_invocable)
            .collect()
    }

    /// Total number of registered skills.
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::types::{Skill, SkillSource};

    fn test_skill(name: &str) -> Skill {
        Skill {
            name: name.to_string(),
            description: format!("Test skill: {}", name),
            source: SkillSource::Bundled,
            when_to_use: None,
            body: "body".to_string(),
            allowed_tools: Vec::new(),
            model: None,
            user_invocable: false,
            argument_hint: None,
            disable_model_invocation: false,
        }
    }

    #[test]
    fn test_find_by_name() {
        let reg = SkillRegistry::from_skills(vec![test_skill("alpha"), test_skill("beta")]);
        assert!(reg.find_by_name("alpha").is_some());
        assert!(reg.find_by_name("gamma").is_none());
    }

    #[test]
    fn test_find_by_trigger() {
        let reg = SkillRegistry::from_skills(vec![test_skill("code-review"), test_skill("deploy")]);
        let results = reg.find_by_trigger("code");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "code-review");
    }

    #[test]
    fn test_duplicates_ignored() {
        let reg = SkillRegistry::from_skills(vec![test_skill("dup"), test_skill("dup")]);
        assert_eq!(reg.len(), 1);
    }
}
