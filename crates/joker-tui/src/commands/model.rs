use joker_config::ConfigStore;

use crate::app::{App, Dialog, DialogKind};

use super::{CommandAction, CommandInfo, CommandResult, SlashCommand};

/// Slash command `/model` for selecting or switching the active model.
pub(super) struct ModelCommand;

impl SlashCommand for ModelCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "model",
            usage: "/model [name]",
            description: "Select a model interactively, or switch directly by name.",
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
                "Cannot switch model while a run is active. Use /cancel first.",
            );
        }
        let Some(model) = args else {
            let options = available_models_for_dialog(app);
            if options.is_empty() {
                return CommandResult {
                    message: Some("No models available for the current provider.".into()),
                    action: None,
                    is_error: false,
                };
            }
            app.dialog = Some(Dialog {
                kind: DialogKind::Model,
                title: format!("Select Model ({})", app.runtime_config.provider_label()),
                options,
                selected: 0,
            });
            return CommandResult {
                message: None,
                action: None,
                is_error: false,
            };
        };
        match app.runtime_config.switch_model(model) {
            Ok(()) => CommandResult::with_message_and_action(
                format!("Switched model to {}", app.runtime_config.provider_label()),
                CommandAction::ConfigChanged,
            ),
            Err(error) => CommandResult::error(error.to_string()),
        }
    }

    fn complete(&self, args_partial: &str) -> Vec<String> {
        let partial = args_partial.to_ascii_lowercase();
        // The completion candidates come from the app's available models,
        // but `complete` doesn't have access to App. Return empty and let
        // the caller handle model completion separately.
        let _ = partial;
        vec![]
    }
}

fn available_models_for_dialog(app: &App) -> Vec<(String, String)> {
    let models = if app.available_models.is_empty() {
        app.runtime_config.available_models()
    } else {
        app.available_models.clone()
    };
    models.into_iter().map(|m| (m.clone(), m)).collect()
}
