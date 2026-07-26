use crate::ContextLimits;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentConfig {
    pub limits: RunLimits,
    pub execution_mode: ExecutionMode,
    pub context_limits: ContextLimits,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryConfig {
    pub max_stream_retries: usize,
    pub max_zero_output_retries: usize,
    pub base_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self { max_stream_retries: 4, max_zero_output_retries: 3, base_delay_ms: 1000 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunLimits {
    pub max_steps: usize,
    pub max_tool_calls: usize,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self { max_steps: 16, max_tool_calls: 64 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionMode {
    Sequential,
    ParallelWhenSafe,
}
