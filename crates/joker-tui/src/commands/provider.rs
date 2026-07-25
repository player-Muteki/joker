use joker_config::ConfigStore;
use joker_provider::{ALIBABA, ANTHROPIC, BAIDU, DEEPSEEK, GOOGLE, MOONSHOT, ZHIPUAI};

use crate::app::{App, Dialog, DialogKind};

use super::{CommandAction, CommandInfo, CommandResult, SlashCommand};

pub(super) struct ProviderCommand;

impl SlashCommand for ProviderCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "provider",
            usage: "/provider [name]",
            description: "Select a provider interactively, or switch directly by name.",
        }
    }

    fn execute(
        &self,
        app: &mut App,
        args: Option<&str>,
        _config_store: &ConfigStore,
    ) -> CommandResult {
        if app.running {
            return CommandResult::error(
                "Cannot switch provider while a run is active. Use /cancel first.",
            );
        }
        let Some(provider) = args else {
            let options = available_providers_for_dialog();
            app.dialog = Some(Dialog {
                kind: DialogKind::Provider,
                title: "Select Provider".into(),
                options,
                selected: 0,
            });
            return CommandResult {
                message: None,
                action: None,
                is_error: false,
            };
        };
        match app.runtime_config.switch_provider(provider) {
            Ok(()) => CommandResult::with_message_and_action(
                format!("Switched provider to {}", app.runtime_config.provider_label()),
                CommandAction::ConfigChanged,
            ),
            Err(error) => CommandResult::error(error.to_string()),
        }
    }

    fn complete(&self, args_partial: &str) -> Vec<String> {
        let partial = args_partial.to_ascii_lowercase();
        PROVIDER_IDS
            .iter()
            .filter(|id| id.to_ascii_lowercase().starts_with(&partial))
            .map(|s| s.to_string())
            .collect()
    }
}

const PROVIDER_IDS: &[&str] = &[
    DEEPSEEK.id, ALIBABA.id, ZHIPUAI.id, MOONSHOT.id, BAIDU.id, ANTHROPIC.id, GOOGLE.id, "scripted",
];

fn available_providers_for_dialog() -> Vec<(String, String)> {
    let providers: [(&str, &str); 8] = [
        ("DeepSeek", DEEPSEEK.id),
        ("Alibaba Cloud (DashScope)", ALIBABA.id),
        ("ZhipuAI (GLM)", ZHIPUAI.id),
        ("Moonshot (Kimi)", MOONSHOT.id),
        ("Baidu (ERNIE)", BAIDU.id),
        ("Anthropic", ANTHROPIC.id),
        ("Google", GOOGLE.id),
        ("Scripted (no AI)", "scripted"),
    ];
    providers
        .iter()
        .map(|(label, id)| (label.to_string(), id.to_string()))
        .collect()
}
