/// A Skill is a bundle of prompt content gated by file path patterns.
///
/// Inspired by claude-code's Skills system and codex's modular instruction
/// packages. Skills can inject system prompts, restrict tool access, and
/// activate automatically when the workspace matches certain file patterns.
#[derive(Clone, Debug)]
pub struct Skill {
    /// Unique name for this skill.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// File path patterns that trigger this skill (e.g. `["*.py", "src/**"]`).
    /// An empty vec means "always active".
    pub paths: Vec<String>,
    /// Prompt content injected into the system prompt when active.
    pub prompt_content: String,
    /// Optional list of allowed tool names. Empty = no restriction.
    pub allowed_tools: Vec<String>,
}

impl Skill {
    /// Create a new `Skill` with the given name and default empty fields.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            paths: Vec::new(),
            prompt_content: String::new(),
            allowed_tools: Vec::new(),
        }
    }

    /// Check whether this skill is active for the given file path.
    #[must_use]
    pub fn is_active_for(&self, _path: &str) -> bool {
        if self.paths.is_empty() {
            return true;
        }
        self.paths.iter().any(|pattern| {
            // Simple glob matching (startswith/endswith for now)
            if let Some(suffix) = pattern.strip_prefix("**/") {
                _path.ends_with(suffix) || _path.contains(suffix)
            } else if let Some(prefix) = pattern.strip_suffix("**") {
                _path.starts_with(prefix.trim_end_matches('/'))
            } else if let Some(suffix) = pattern.strip_prefix('*') {
                _path.ends_with(suffix)
            } else if pattern.ends_with('*') {
                _path.starts_with(pattern.trim_end_matches('*'))
            } else {
                _path == pattern
            }
        })
    }

    /// Build the system prompt fragment for this skill.
    #[must_use]
    pub fn system_prompt(&self) -> String {
        if self.prompt_content.is_empty() {
            String::new()
        } else {
            format!("[Skill: {}]\n{}", self.name, self.prompt_content)
        }
    }
}

/// A registry of skills, loaded from multiple sources.
#[derive(Clone, Debug, Default)]
pub struct SkillRegistry {
    skills: Vec<Skill>,
}

impl SkillRegistry {
    /// Create a new empty `SkillRegistry`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a skill to this registry.
    pub fn register(&mut self, skill: Skill) {
        self.skills.push(skill);
    }

    /// Get all skills active for the given path.
    #[must_use]
    pub fn active_for(&self, path: &str) -> Vec<&Skill> {
        self.skills
            .iter()
            .filter(|s| s.is_active_for(path))
            .collect()
    }

    /// Build the full system prompt fragment from all active skills.
    #[must_use]
    pub fn system_prompt_for(&self, path: &str) -> String {
        self.active_for(path)
            .iter()
            .filter_map(|s| {
                let p = s.system_prompt();
                if p.is_empty() { None } else { Some(p) }
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_glob_matching() {
        let skill = Skill {
            name: "rust".into(),
            description: String::new(),
            paths: vec!["*.rs".into(), "src/**".into()],
            prompt_content: "Follow Rust idioms.".into(),
            allowed_tools: Vec::new(),
        };
        assert!(skill.is_active_for("main.rs"));
        assert!(skill.is_active_for("src/lib.rs"));
        assert!(!skill.is_active_for("README.md"));

        let registry = SkillRegistry::new();
        // no skills = empty prompt
        assert!(registry.system_prompt_for("main.rs").is_empty());
    }

    #[test]
    fn skill_active_when_no_paths() {
        let skill = Skill {
            name: "always".into(),
            description: String::new(),
            paths: Vec::new(),
            prompt_content: "Always active.".into(),
            allowed_tools: Vec::new(),
        };
        assert!(skill.is_active_for("any/file.txt"));
    }

    #[test]
    fn skill_system_prompt_format() {
        let skill = Skill {
            name: "test".into(),
            description: String::new(),
            paths: vec!["*".into()],
            prompt_content: "Test prompt.".into(),
            allowed_tools: Vec::new(),
        };
        let prompt = skill.system_prompt();
        assert!(prompt.contains("[Skill: test]"));
        assert!(prompt.contains("Test prompt."));
    }
}
