//! Context building and compaction system.
//!
//! Builders implement [`ContextBuilder`] to produce a [`BuiltContext`] from a
//! [`Conversation`].  Composition wrappers add fixed-window truncation
//! ([`FixedWindowContextBuilder`]), LLM-style summary ([`SummaryContextBuilder`]),
//! system-prefix injection ([`PrefixedContextBuilder`]), and multi-level
//! compaction ([`CompactingContextBuilder`]).
//!
//! Compaction strategies (soft / compact / force / micro) are driven by
//! token-count thresholds ([`ContextThresholds`]) and a [`CompactionLevel`]
//! classifier.  The [`micro_dedup_messages`] helper replaces repeated large
//! tool results with stubs without calling an LLM.

use std::sync::Arc;

use thiserror::Error;

use crate::{
    Content, Conversation, Event, Message, NoopObserver, Observer, error::BoxFutureResult,
};

/// Future returned by [`ContextBuilder::build`].
pub type ContextFuture<'a> = BoxFutureResult<'a, BuiltContext, ContextError>;

/// Produces a [`BuiltContext`] from a [`Conversation`] slice and [`ContextLimits`].
///
/// Implementations may truncate, summarize, or reorder messages.  The trait is
/// object-safe and all public builders implement it.
pub trait ContextBuilder: Send + Sync {
    /// Build a context, returning a [`BuiltContext`] or a [`ContextError`].
    fn build<'a>(&'a self, input: ContextInput<'a>) -> ContextFuture<'a>;
}

/// Input passed to [`ContextBuilder::build`].
#[derive(Clone, Copy, Debug)]
pub struct ContextInput<'a> {
    /// Conversation to build context from.
    pub conversation: &'a Conversation,
    /// Hard limits the builder must not exceed.
    pub limits: ContextLimits,
}

/// Result produced by a [`ContextBuilder`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltContext {
    /// Messages ready for the LLM request.
    pub messages: Vec<Message>,
}

/// Hard limits enforced by every [`ContextBuilder`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextLimits {
    /// Maximum number of messages allowed.
    pub max_messages: usize,
    /// Maximum total text bytes across all messages.
    pub max_text_bytes: usize,
    /// Maximum total tool-result bytes across all messages.
    pub max_tool_result_bytes: usize,
}

impl Default for ContextLimits {
    fn default() -> Self {
        Self {
            max_messages: 64,
            max_text_bytes: 64 * 1024,
            max_tool_result_bytes: 64 * 1024,
        }
    }
}

/// Errors returned by context building and limit enforcement.
#[derive(Debug, Error)]
pub enum ContextError {
    /// A hard limit (messages, text bytes, or tool-result bytes) was exceeded.
    #[error("context limit exceeded: {0}")]
    LimitExceeded(&'static str),
}

/// Builder that passes through the full [`Conversation`] unchanged (subject to
/// [`ContextLimits`]).
#[derive(Default)]
pub struct PassthroughContextBuilder;

impl ContextBuilder for PassthroughContextBuilder {
    fn build<'a>(&'a self, input: ContextInput<'a>) -> ContextFuture<'a> {
        Box::pin(async move {
            let messages = input.conversation.messages().to_vec();
            enforce_limits(&messages, input.limits)?;
            Ok(BuiltContext { messages })
        })
    }
}

/// Builder that keeps only the *N* most recent messages.
#[derive(Clone, Debug)]
pub struct FixedWindowContextBuilder {
    max_messages: usize,
}

impl FixedWindowContextBuilder {
    /// Create a builder that keeps up to `max_messages` recent messages.
    #[must_use]
    pub fn new(max_messages: usize) -> Self {
        Self { max_messages }
    }
}

impl ContextBuilder for FixedWindowContextBuilder {
    fn build<'a>(&'a self, input: ContextInput<'a>) -> ContextFuture<'a> {
        Box::pin(async move {
            let max_messages = self.max_messages.min(input.limits.max_messages);
            let messages = input.conversation.messages();
            let start = messages.len().saturating_sub(max_messages);
            let built = messages[start..].to_vec();
            enforce_limits(&built, input.limits)?;
            Ok(BuiltContext { messages: built })
        })
    }
}

/// Context builder that uses a summary string when conversations grow large.
///
/// Keeps the most recent messages and prepends a summary of earlier ones
/// as a system message when the total message count exceeds the threshold.
pub struct SummaryContextBuilder {
    max_recent_messages: usize,
    inner: Box<dyn ContextBuilder>,
}

impl SummaryContextBuilder {
    /// Wrap `inner` so that older messages are summarized when the conversation
    /// grows past `max_recent_messages`.
    #[must_use]
    pub fn new(max_recent_messages: usize, inner: Box<dyn ContextBuilder>) -> Self {
        Self {
            max_recent_messages,
            inner,
        }
    }

    /// Summarize a conversation into a compact string.
    /// This is a heuristic summary — in production you'd use an LLM call.
    pub fn summarize_conversation(conversation: &Conversation) -> String {
        let messages = conversation.messages();
        if messages.is_empty() {
            return String::new();
        }

        let mut parts: Vec<String> = Vec::new();
        let mut user_msgs = 0usize;
        let mut assistant_msgs = 0usize;
        let mut tool_calls = 0usize;
        let mut tool_results = 0usize;

        for msg in messages {
            match msg.role {
                crate::Role::User => user_msgs += 1,
                crate::Role::Assistant => {
                    assistant_msgs += 1;
                    for content in &msg.content {
                        if matches!(content, Content::ToolCall(_)) {
                            tool_calls += 1;
                        }
                    }
                }
                crate::Role::Tool => tool_results += 1,
                crate::Role::System => {}
            }
        }

        // Extract first user message as context clue
        let first_user = messages
            .iter()
            .find(|m| m.role == crate::Role::User)
            .and_then(|m| {
                m.content.iter().find_map(|c| match c {
                    Content::Text(t) => Some(t.text.clone()),
                    _ => None,
                })
            })
            .unwrap_or_default();

        parts.push(format!(
            "This conversation has {} user messages, {} assistant messages, {} tool calls, and {} tool results.",
            user_msgs, assistant_msgs, tool_calls, tool_results
        ));

        if !first_user.is_empty() {
            parts.push(format!(
                "The initial request was: \"{}\"",
                Self::truncate_text(&first_user, 200)
            ));
        }

        parts.push("Earlier messages have been summarized. Key context is preserved above.".into());
        parts.join("\n")
    }

    fn truncate_text(text: &str, max: usize) -> String {
        if text.len() <= max {
            text.to_string()
        } else {
            format!("{}...", &text[..max])
        }
    }
}

impl ContextBuilder for SummaryContextBuilder {
    fn build<'a>(&'a self, input: ContextInput<'a>) -> ContextFuture<'a> {
        Box::pin(async move {
            let messages = input.conversation.messages();

            // If conversation is small enough, passthrough to inner builder
            if messages.len() <= self.max_recent_messages {
                return self.inner.build(input).await;
            }

            // Build a summary of older messages
            let cutoff = messages.len() - self.max_recent_messages;
            let older_msgs = &messages[..cutoff];
            let recent_msgs = &messages[cutoff..];

            // Create a temporary conversation for summarization
            let older_conv = Conversation::from_messages(older_msgs.to_vec());
            let summary = Self::summarize_conversation(&older_conv);

            // Prepend summary as system message
            let mut built = Vec::new();
            if !summary.is_empty() {
                built.push(Message {
                    role: crate::Role::System,
                    content: vec![Content::text(format!(
                        "[Summary of earlier conversation]:\n{summary}"
                    ))],
                });
            }
            built.extend_from_slice(recent_msgs);

            enforce_limits(&built, input.limits)?;
            Ok(BuiltContext { messages: built })
        })
    }
}

fn enforce_limits(messages: &[Message], limits: ContextLimits) -> Result<(), ContextError> {
    if messages.len() > limits.max_messages {
        return Err(ContextError::LimitExceeded("messages"));
    }

    let mut text_bytes = 0usize;
    let mut tool_result_bytes = 0usize;
    for message in messages {
        for content in &message.content {
            match content {
                Content::Text(text) => text_bytes += text.text.len(),
                Content::Reasoning(reasoning) => text_bytes += reasoning.text.len(),
                Content::ToolResult(result) => tool_result_bytes += result.output.to_string().len(),
                Content::ToolCall(call) => text_bytes += call.arguments.to_string().len(),
            }
        }
    }

    if text_bytes > limits.max_text_bytes {
        return Err(ContextError::LimitExceeded("text bytes"));
    }
    if tool_result_bytes > limits.max_tool_result_bytes {
        return Err(ContextError::LimitExceeded("tool result bytes"));
    }
    Ok(())
}

// ── Token estimation ─────────────────────────────────────────────────────

/// Heuristic token count estimator.
///
/// Uses ~4 characters per token for English text. Not exact but fast and
/// sufficient for triggering compaction thresholds.
#[must_use]
pub fn estimate_tokens(messages: &[Message]) -> usize {
    let mut total_chars = 0usize;
    for message in messages {
        for content in &message.content {
            total_chars += match content {
                Content::Text(t) => t.text.len(),
                Content::Reasoning(r) => r.text.len(),
                Content::ToolCall(c) => c.arguments.to_string().len() + c.name.len(),
                Content::ToolResult(r) => r.output.to_string().len(),
            };
        }
    }
    total_chars / 4
}

/// Token-count thresholds that drive context compaction.
///
/// See [`CompactionLevel::from_tokens`] for how thresholds map to levels.
#[derive(Clone, Copy, Debug)]
pub struct ContextThresholds {
    /// Token count at which to notify the user (soft limit).
    pub soft_tokens: usize,
    /// Token count at which to apply LLM summarization.
    pub compact_tokens: usize,
    /// Token count at which to force-truncate.
    pub force_tokens: usize,
    /// Number of recent messages to preserve during compact/force.
    pub recent_messages: usize,
}

impl Default for ContextThresholds {
    fn default() -> Self {
        Self {
            soft_tokens: 48_000,    // 50% of 96K
            compact_tokens: 76_800, // 80% of 96K
            force_tokens: 86_400,   // 90% of 96K
            recent_messages: 8,
        }
    }
}

/// Compaction level determined by evaluating token count against
/// [`ContextThresholds`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactionLevel {
    /// Under all thresholds — no action needed.
    None,
    /// Micro dedup — replace repeated large outputs with stubs.
    Micro,
    /// Soft limit hit — notify user, no compression.
    Soft,
    /// Compact — summarize older messages, keep recent window.
    Compact,
    /// Force — aggressive truncation.
    Force,
}

impl CompactionLevel {
    /// Classify a token count into a [`CompactionLevel`] using the given
    /// [`ContextThresholds`].
    #[must_use]
    pub fn from_tokens(tokens: usize, thresholds: &ContextThresholds) -> Self {
        if tokens >= thresholds.force_tokens {
            Self::Force
        } else if tokens >= thresholds.compact_tokens {
            Self::Compact
        } else if tokens >= thresholds.soft_tokens {
            Self::Soft
        } else {
            Self::None
        }
    }
}

// ── Micro dedup: replace repeated large tool results with stubs ──────────

/// Replace repeated read_file tool results over 2000 bytes with a stub
/// referencing the first occurrence. Keeps context small when the same
/// file is read multiple times.
#[must_use]
pub fn micro_dedup_messages(messages: &mut [Message]) -> usize {
    use std::collections::HashMap;
    let mut dedup_count = 0usize;
    let mut seen_files: HashMap<String, usize> = HashMap::new();
    let stub_threshold = 2000usize;

    for msg in messages.iter_mut() {
        if msg.role != crate::Role::Tool {
            continue;
        }
        for content in &mut msg.content {
            if let Content::ToolResult(result) = content {
                let output_str = result.output.to_string();
                if output_str.len() < stub_threshold {
                    continue;
                }
                // Try to detect read_file results by looking for "path" and "content"
                let detected_path = output_str
                    .lines()
                    .next()
                    .and_then(|l| l.strip_prefix("path:").map(String::from))
                    .or_else(|| {
                        serde_json::from_str::<serde_json::Value>(&output_str)
                            .ok()
                            .and_then(|v| v.get("path").and_then(|p| p.as_str().map(String::from)))
                    })
                    .or_else(|| {
                        output_str
                            .lines()
                            .find(|l| l.trim().starts_with('"'))
                            .map(|l| l.trim_matches('"').to_string())
                    });
                if let Some(path) = detected_path {
                    if let Some(&first_idx) = seen_files.get(&path) {
                        // Replace with stub
                        result.output = serde_json::Value::String(format!(
                            "[stub] Same file content as tool result #{} ({} bytes). See earlier result.",
                            first_idx,
                            output_str.len()
                        ));
                        dedup_count += 1;
                    } else {
                        seen_files.insert(path, seen_files.len() + 1);
                    }
                }
            }
        }
    }
    dedup_count
}

// ── CompactingContextBuilder ──────────────────────────────────────────────

/// Wraps an inner builder and applies compaction strategies based on token
/// thresholds.
pub struct CompactingContextBuilder {
    thresholds: ContextThresholds,
    #[allow(dead_code)]
    inner: Box<dyn ContextBuilder>,
    observer: Arc<dyn Observer>,
}

impl CompactingContextBuilder {
    /// Wrap an inner builder with default [`ContextThresholds`] and a
    /// [`NoopObserver`].
    #[must_use]
    pub fn new(inner: Box<dyn ContextBuilder>) -> Self {
        Self {
            thresholds: ContextThresholds::default(),
            inner,
            observer: Arc::new(NoopObserver),
        }
    }

    /// Override the compaction thresholds.
    #[must_use]
    pub fn with_thresholds(mut self, thresholds: ContextThresholds) -> Self {
        self.thresholds = thresholds;
        self
    }

    /// Attach an observer for compaction lifecycle events.
    #[must_use]
    pub fn with_observer(mut self, observer: Arc<dyn Observer>) -> Self {
        self.observer = observer;
        self
    }
}

impl ContextBuilder for CompactingContextBuilder {
    fn build<'a>(&'a self, input: ContextInput<'a>) -> ContextFuture<'a> {
        Box::pin(async move {
            let mut messages = input.conversation.messages().to_vec();

            // Micro: always active — dedup repeated large file reads
            let _deduped = micro_dedup_messages(&mut messages);
            let tokens = estimate_tokens(&messages);
            let level = CompactionLevel::from_tokens(tokens, &self.thresholds);

            match level {
                CompactionLevel::None | CompactionLevel::Micro => {
                    // Passthrough with micro dedup already applied
                    enforce_limits(&messages, input.limits)?;
                    return Ok(BuiltContext { messages });
                }
                CompactionLevel::Soft => {
                    // Soft: return full context but signal the caller
                    // (the caller checks CompactionLevel to notify user)
                    enforce_limits(&messages, input.limits)?;
                    return Ok(BuiltContext { messages });
                }
                CompactionLevel::Compact => {
                    let tokens_before = estimate_tokens(&messages);
                    let _ = self
                        .observer
                        .observe(Event::CompactionStarted {
                            trigger: "threshold".into(),
                            current_tokens: tokens_before,
                            threshold: self.thresholds.compact_tokens,
                        })
                        .await;

                    // Keep summary of old + recent window
                    let recent = self.thresholds.recent_messages;
                    if messages.len() > recent {
                        let cut = messages.len() - recent;
                        let older = messages[..cut].to_vec();
                        let recent_msgs = messages[cut..].to_vec();

                        let older_conv = Conversation::from_messages(older);
                        let summary = SummaryContextBuilder::summarize_conversation(&older_conv);

                        let mut built = Vec::new();
                        if !summary.is_empty() {
                            built.push(Message {
                                role: crate::Role::System,
                                content: vec![Content::text(format!(
                                    "[Context summary — earlier messages compacted ({} tokens)]:\n{summary}",
                                    estimate_tokens(&recent_msgs)
                                ))],
                            });
                        }
                        built.extend(recent_msgs);
                        enforce_limits(&built, input.limits)?;

                        let tokens_after = estimate_tokens(&built);
                        let _ = self
                            .observer
                            .observe(Event::CompactionDone {
                                tokens_before,
                                tokens_after,
                            })
                            .await;
                        return Ok(BuiltContext { messages: built });
                    }
                }
                CompactionLevel::Force => {
                    let tokens_before = estimate_tokens(&messages);
                    let _ = self
                        .observer
                        .observe(Event::CompactionStarted {
                            trigger: "force".into(),
                            current_tokens: tokens_before,
                            threshold: self.thresholds.force_tokens,
                        })
                        .await;

                    // Force: only keep system prompts + last K messages
                    let recent = self.thresholds.recent_messages.min(4);
                    let system_msgs: Vec<Message> = messages
                        .iter()
                        .filter(|m| m.role == crate::Role::System)
                        .cloned()
                        .collect();
                    let recent_start = messages.len().saturating_sub(recent);
                    let recent_msgs: Vec<Message> = messages[recent_start..]
                        .iter()
                        .filter(|m| m.role != crate::Role::System)
                        .cloned()
                        .collect();

                    let mut built = system_msgs;
                    if !recent_msgs.is_empty() {
                        built.push(Message {
                            role: crate::Role::System,
                            content: vec![Content::text(format!(
                                "[Context force-compacted. Showing last {} messages. Earlier content summarized above.]",
                                recent_msgs.len()
                            ))],
                        });
                    }
                    built.extend(recent_msgs);
                    enforce_limits(&built, input.limits)?;

                    let tokens_after = estimate_tokens(&built);
                    let _ = self
                        .observer
                        .observe(Event::CompactionDone {
                            tokens_before,
                            tokens_after,
                        })
                        .await;
                    return Ok(BuiltContext { messages: built });
                }
            }
            // Fallback
            enforce_limits(&messages, input.limits)?;
            Ok(BuiltContext { messages })
        })
    }
}

// ── PrefixedContextBuilder ────────────────────────────────────────────────

/// Prepends a system message to the built context before delegating to
/// the inner builder.
pub struct PrefixedContextBuilder {
    prefix: String,
    inner: Box<dyn ContextBuilder>,
}

impl PrefixedContextBuilder {
    /// Create a builder that inserts a system message with `prefix` before
    /// delegating to `inner`.
    #[must_use]
    pub fn new(prefix: impl Into<String>, inner: Box<dyn ContextBuilder>) -> Self {
        Self {
            prefix: prefix.into(),
            inner,
        }
    }
}

impl ContextBuilder for PrefixedContextBuilder {
    fn build<'a>(&'a self, input: ContextInput<'a>) -> ContextFuture<'a> {
        Box::pin(async move {
            let mut built = self.inner.build(input).await?;
            if !self.prefix.is_empty() {
                built.messages.insert(
                    0,
                    Message {
                        role: crate::Role::System,
                        content: vec![Content::text(self.prefix.clone())],
                    },
                );
            }
            Ok(built)
        })
    }
}

/// Assemble a system prompt from an agent profile, project context, and memory.
///
/// The prompt is built by concatenating:
/// 1. The project's base context (`project_context`, if any)
/// 2. The agent's constraint file content
/// 3. Any active memory notes
///
/// The result is intended to be prepended via `PrefixedContextBuilder`.
#[must_use]
pub fn assemble_system_prompt(
    agent_name: &str,
    project_context: Option<&str>,
    memory: Option<&str>,
) -> String {
    crate::AgentProfileCatalog::new(std::path::PathBuf::new()).system_prompt(
        agent_name,
        project_context,
        memory,
    )
}
