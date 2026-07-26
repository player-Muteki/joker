//! Command registry with fuzzy-matching autocomplete.
//!
//! [`CommandRegistry`] stores registered [`SlashCommand`] handlers and
//! provides dispatch, autocomplete (prefix + fuzzy), and help-text
//! generation.

use std::sync::Arc;

use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use joker_config::ConfigStore;

use crate::app::App;

use super::{CommandInfo, CommandResult, SlashCommand};

struct CommandEntry {
    info: CommandInfo,
    handler: Arc<dyn SlashCommand>,
}

/// Registry of slash commands with fuzzy-matching autocomplete.
pub struct CommandRegistry {
    entries: Vec<CommandEntry>,
    matcher: SkimMatcherV2,
}

impl CommandRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            matcher: SkimMatcherV2::default(),
        }
    }

    /// Register a command. Order matters for match ties — first registered wins.
    pub fn register(&mut self, command: Arc<dyn SlashCommand>) {
        let info = command.info();
        self.entries.push(CommandEntry {
            info,
            handler: command,
        });
    }

    /// Execute a command by parsing the input string.
    pub fn execute(
        &self,
        input: &str,
        app: &mut App,
        config_store: &ConfigStore,
    ) -> CommandResult {
        let trimmed = input.trim().trim_start_matches('/');
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let name = parts.next().unwrap_or_default().to_ascii_lowercase();
        let args = parts
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty());

        if name.is_empty() {
            return self.build_help(args);
        }

        // Exact match first
        for entry in &self.entries {
            if entry.info.name == name {
                return entry.handler.execute(app, args, config_store);
            }
        }

        // Unknown — return error with suggestions
        let suggestions = self.suggestions(&name);
        let suggestion_strs: Vec<String> = suggestions
            .into_iter()
            .map(|info| format!("/{}", info.name))
            .collect();

        if suggestion_strs.is_empty() {
            CommandResult::error(format!("Unknown command: /{name}. Type /help."))
        } else {
            CommandResult::error(format!(
                "Unknown command: /{name}. Did you mean {}?",
                suggestion_strs.join(", ")
            ))
        }
    }

    /// Return completion candidates for a partial slash-command input.
    pub fn complete(&self, input: &str) -> Vec<CommandInfo> {
        let trimmed = input.trim().trim_start_matches('/');
        let has_args = trimmed.contains(char::is_whitespace);
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let command_name = parts.next().unwrap_or_default().to_ascii_lowercase();
        let args_partial = parts.next().unwrap_or_default();

        if has_args
            && let Some(entry) = self.entries.iter().find(|entry| entry.info.name == command_name)
        {
            let completions = entry.handler.complete(args_partial);
            return completions
                .into_iter()
                .map(|completion| CommandInfo {
                    name: Box::leak(format!("{} {}", entry.info.name, completion).into_boxed_str()),
                    usage: entry.info.usage,
                    description: entry.info.description,
                })
                .collect();
        }

        let query = command_name;

        if query.is_empty() {
            return self.entries.iter().map(|e| e.info).collect();
        }

        let mut scored: Vec<(i64, CommandInfo)> = self
            .entries
            .iter()
            .filter_map(|entry| {
                let score = self.matcher.fuzzy_match(entry.info.name, &query)?;
                Some((score, entry.info))
            })
            .collect();

        scored.sort_by_key(|b| std::cmp::Reverse(b.0));
        scored.into_iter().take(8).map(|(_, info)| info).collect()
    }

    /// Return matching command infos for suggestion display (prefix + fuzzy fallback).
    pub fn suggestions(&self, query: &str) -> Vec<CommandInfo> {
        let query = query.to_ascii_lowercase();
        if query.is_empty() {
            return self.entries.iter().map(|e| e.info).collect();
        }

        // Prefer prefix matches
        let prefix_matches: Vec<CommandInfo> = self
            .entries
            .iter()
            .filter(|entry| entry.info.name.starts_with(&query))
            .map(|e| e.info)
            .collect();

        if !prefix_matches.is_empty() {
            return prefix_matches.into_iter().take(6).collect();
        }

        // Fallback to fuzzy
        let mut scored: Vec<(i64, CommandInfo)> = self
            .entries
            .iter()
            .filter_map(|entry| {
                let score = self.matcher.fuzzy_match(entry.info.name, &query)?;
                Some((score, entry.info))
            })
            .collect();
        scored.sort_by_key(|b| std::cmp::Reverse(b.0));
        scored.into_iter().take(6).map(|(_, info)| info).collect()
    }

    /// Iterate all registered command infos (for help / listing).
    pub fn list(&self) -> Vec<CommandInfo> {
        self.entries.iter().map(|e| e.info).collect()
    }

    fn build_help(&self, args: Option<&str>) -> CommandResult {
        if let Some(name) = args {
            if let Some(entry) = self.entries.iter().find(|e| e.info.name == name) {
                return CommandResult::message(format!(
                    "{}\n{}",
                    entry.info.usage, entry.info.description
                ));
            }
            return CommandResult::error(format!("Unknown command: /{name}"));
        }

        let all = self.list();
        CommandResult::message(
            all.iter()
                .map(|info| format!("{:<36} {}", info.usage, info.description))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}
