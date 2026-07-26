use crate::ContextLimits;

/// Top-level configuration for an [`Agent`](crate::Agent).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentConfig {
    /// Step and tool-call limits for a single run.
    pub limits: RunLimits,
    /// Whether tools may execute in parallel when safe.
    pub execution_mode: ExecutionMode,
    /// Token and window limits for context building.
    pub context_limits: ContextLimits,
    /// Retry strategy for model-stream failures and empty responses.
    pub retry: RetryConfig,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            limits: RunLimits::default(),
            execution_mode: ExecutionMode::Sequential,
            context_limits: ContextLimits::default(),
            retry: RetryConfig::default(),
        }
    }
}

/// Retry strategy for model-stream errors and empty responses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryConfig {
    /// Maximum number of retries on stream-initialization errors.
    pub max_stream_retries: usize,
    /// Maximum number of retries when the model returns no output.
    pub max_zero_output_retries: usize,
    /// Base delay (ms) for exponential backoff.
    pub base_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self { max_stream_retries: 4, max_zero_output_retries: 3, base_delay_ms: 1000 }
    }
}

/// Limits on steps and tool calls per run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunLimits {
    /// Maximum number of turns (model requests) per run.
    pub max_steps: usize,
    /// Maximum total tool calls across all turns.
    pub max_tool_calls: usize,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self { max_steps: 16, max_tool_calls: 64 }
    }
}

/// Whether tool calls within a turn run sequentially or in parallel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionMode {
    /// Execute tool calls one at a time.
    Sequential,
    /// Execute parallel-safe tool calls concurrently.
    ParallelWhenSafe,
}
