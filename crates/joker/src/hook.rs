use std::sync::Arc;

use crate::{Conversation, Message, ToolInvocation, ToolOutput};

/// Lifecycle hooks for customizing agent behavior.
///
/// Inspired by gemini-cli's `IBeforeToolHook` / `IAfterToolHook` and
/// OpenCode's `Plugin.Service` event hooks.
pub trait Hook: Send + Sync {
    /// Called before a tool is invoked. Return `Err` to block the call.
    fn before_tool_call(
        &self,
        _invocation: &ToolInvocation,
        _conversation: &Conversation,
    ) -> Result<(), String> {
        Ok(())
    }

    /// Called after a tool result is received. Can modify the output.
    fn after_tool_call(
        &self,
        _invocation: &ToolInvocation,
        _output: &mut ToolOutput,
        _conversation: &mut Conversation,
    ) {
    }

    /// Called before the provider request is sent.
    fn before_provider_request(&self, _messages: &mut Vec<Message>) {}

    /// Called on session start.
    fn on_session_start(&self, _agent_name: &str) {}

    /// Called on session end.
    fn on_session_end(&self, _agent_name: &str) {}
}

/// A registry of hooks that are all executed in order.
#[derive(Default)]
pub struct HookRegistry {
    hooks: Vec<Arc<dyn Hook>>,
}

impl HookRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, hook: Arc<dyn Hook>) {
        self.hooks.push(hook);
    }

    pub fn before_tool_call(
        &self,
        invocation: &ToolInvocation,
        conversation: &Conversation,
    ) -> Result<(), String> {
        for hook in &self.hooks {
            hook.before_tool_call(invocation, conversation)?;
        }
        Ok(())
    }

    pub fn after_tool_call(
        &self,
        invocation: &ToolInvocation,
        output: &mut ToolOutput,
        conversation: &mut Conversation,
    ) {
        for hook in &self.hooks {
            hook.after_tool_call(invocation, output, conversation);
        }
    }

    pub fn before_provider_request(&self, messages: &mut Vec<Message>) {
        for hook in &self.hooks {
            hook.before_provider_request(messages);
        }
    }

    pub fn on_session_start(&self, agent_name: &str) {
        for hook in &self.hooks {
            hook.on_session_start(agent_name);
        }
    }

    pub fn on_session_end(&self, agent_name: &str) {
        for hook in &self.hooks {
            hook.on_session_end(agent_name);
        }
    }
}
