use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::error::BoxFutureResult;

pub type SearchFuture<'a> = BoxFutureResult<'a, Vec<SearchResult>, WebSearchError>;

/// Abstract interface for web search providers.
///
/// Each backend (DuckDuckGo, Bing, Tavily, etc.) implements this trait.
/// The trait is deliberately simple — richer capabilities (recency, domain
/// filtering, locale) are added as needed.
pub trait WebSearch: Send + Sync {
    /// Execute a search query and return up to `max_results` results.
    fn search(&self, query: &str, max_results: usize) -> SearchFuture<'_>;
}

/// One search result with citation-friendly fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResult {
    /// Display title of the result.
    pub title: String,
    /// Absolute URL of the result.
    pub url: String,
    /// A short text snippet summarising the result.
    pub snippet: String,
}

impl fmt::Display for SearchResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} — {}\n  {}", self.title, self.url, self.snippet)
    }
}

/// Errors that can occur during a web search.
#[derive(Debug, Error)]
pub enum WebSearchError {
    #[error("search request failed: {0}")]
    Request(String),
    #[error("search backend returned an error: {0}")]
    Backend(String),
    #[error("invalid query: {0}")]
    InvalidQuery(String),
    #[error("rate limited")]
    RateLimited,
}

impl From<WebSearchError> for crate::ToolError {
    fn from(error: WebSearchError) -> Self {
        crate::ToolError::Execution(error.to_string())
    }
}
