use std::sync::Arc;

use joker::{
    SearchFuture, SearchResult, Tool, ToolAnnotations, ToolDefinition, ToolError, ToolExecution,
    ToolFuture, ToolInvocation, ToolName, ToolOutput, WebSearch, WebSearchError,
};
use serde_json::json;

// ── DuckDuckGo Search Backend ───────────────────────────────────────────

/// DuckDuckGo search backend using the HTML search endpoint.
/// No API key required.
pub struct DuckDuckGoSearch {
    client: reqwest::Client,
}

impl DuckDuckGoSearch {
    pub fn new() -> Result<Self, WebSearchError> {
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (compatible; JokerBot/1.0)")
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| WebSearchError::Request(e.to_string()))?;
        Ok(Self { client })
    }

    /// Parse DuckDuckGo HTML search results.
    /// Extracts title, URL, and snippet from the HTML.
    fn parse_results(html: &str, max_results: usize) -> Vec<SearchResult> {
        let mut results = Vec::new();

        // DuckDuckGo HTML results are in <div class="result"> blocks.
        // We split on "result__a" (title links) and "result__snippet" (snippets)
        // to extract structured results.
        let mut pos = 0usize;
        let html_bytes = html.as_bytes();

        while results.len() < max_results {
            // Find next result block
            let result_start = match Self::find_substring(html_bytes, pos, r#"class="result__a""#) {
                Some(p) => p,
                None => break,
            };

            // Find the href by searching forward from result_start to the next >
            let a_tag_end = match Self::find_substring(html_bytes, result_start, ">") {
                Some(p) => p,
                None => {
                    pos = result_start + 1;
                    continue;
                }
            };

            let href_start = match Self::find_substring(html_bytes, result_start, "href=\"") {
                Some(p) => p + 6, // skip href="
                None => {
                    pos = result_start + 1;
                    continue;
                }
            };

            let href_end = match Self::find_substring(html_bytes, href_start, "\"") {
                Some(p) => p,
                None => {
                    pos = result_start + 1;
                    continue;
                }
            };

            // Validate href is within the same <a> tag
            if href_end > a_tag_end {
                pos = result_start + 1;
                continue;
            }

            let url = String::from_utf8_lossy(&html_bytes[href_start..href_end])
                .replace("&amp;", "&")
                .to_string();

            // Decode DuckDuckGo redirect URLs
            let url = if url.starts_with("//") {
                format!("https:{}", url)
            } else if url.contains("/l/uddg=") {
                // Extract actual URL from redirect
                if let Some(encoded) = url.split("uddg=").nth(1) {
                    if let Some(end) = encoded.find('&') {
                        urlencoding_decode(&encoded[..end]).unwrap_or(url.clone())
                    } else {
                        urlencoding_decode(encoded).unwrap_or(url.clone())
                    }
                } else {
                    url
                }
            } else {
                url
            };

            // Find the title text after the <a> tag
            let title_start = match Self::find_substring(html_bytes, result_start, ">") {
                Some(p) => p + 1,
                None => {
                    pos = result_start + 1;
                    continue;
                }
            };

            let title_end = match Self::find_substring(html_bytes, title_start, "</a>") {
                Some(p) => p,
                None => {
                    pos = result_start + 1;
                    continue;
                }
            };

            let title = String::from_utf8_lossy(&html_bytes[title_start..title_end])
                .replace("&amp;", "&")
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&#x27;", "'")
                .to_string();

            // Find snippet
            let snippet = if let Some(snip_start) =
                Self::find_substring(html_bytes, title_end, r#"class="result__snippet""#)
            {
                if let Some(st) = Self::find_substring(html_bytes, snip_start, ">") {
                    let s_start = st + 1;
                    let s_end = Self::find_substring(html_bytes, s_start, "</a>")
                        .unwrap_or(s_start + 200);
                    String::from_utf8_lossy(&html_bytes[s_start..s_end])
                        .replace("&amp;", "&")
                        .replace("&lt;", "<")
                        .replace("&gt;", ">")
                        .replace("&#x27;", "'")
                        .trim()
                        .to_string()
                } else {
                    String::new()
                }
            } else {
                String::new()
            };

            results.push(SearchResult {
                title,
                url,
                snippet,
            });
            pos = title_end + 1;
        }

        results
    }

    fn find_substring(haystack: &[u8], start: usize, needle: &str) -> Option<usize> {
        let needle_bytes = needle.as_bytes();
        haystack[start..]
            .windows(needle_bytes.len())
            .position(|w| w == needle_bytes)
            .map(|p| start + p)
    }

}

impl WebSearch for DuckDuckGoSearch {
    fn search(&self, query: &str, max_results: usize) -> SearchFuture<'_> {
        let query = query.to_string();
        Box::pin(async move {
            let url = format!(
                "https://html.duckduckgo.com/html/?q={}",
                urlencoding(&query)
            );

            let response = self
                .client
                .get(&url)
                .header("Accept", "text/html")
                .send()
                .await
                .map_err(|e| WebSearchError::Request(e.to_string()))?;

            let status = response.status();
            if !status.is_success() {
                return if status.as_u16() == 429 {
                    Err(WebSearchError::RateLimited)
                } else {
                    Err(WebSearchError::Backend(format!(
                        "duckduckgo returned {status}"
                    )))
                };
            }

            let html = response
                .text()
                .await
                .map_err(|e| WebSearchError::Request(e.to_string()))?;

            let results = Self::parse_results(&html, max_results);
            Ok(results)
        })
    }
}

// ── WebSearchTool (Tool wrapper) ────────────────────────────────────────

/// The `web_search` tool that uses a `WebSearch` backend.
pub struct WebSearchTool {
    backend: Arc<dyn WebSearch>,
}

impl WebSearchTool {
    pub fn new(backend: Arc<dyn WebSearch>) -> Self {
        Self { backend }
    }
}

impl Tool for WebSearchTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: ToolName::new("web_search"),
            description: "Search the web for current information. Returns up to 10 results with title, URL, and snippet.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query." },
                    "max_results": { "type": "integer", "minimum": 1, "maximum": 10, "default": 5 }
                },
                "required": ["query"]
            }),
            annotations: ToolAnnotations {
                execution: ToolExecution::Sequential,
                mutating: false,
                timeout: Some(std::time::Duration::from_secs(20)),
            },
        }
    }

    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_> {
        let backend = self.backend.clone();
        Box::pin(async move {
            let query = invocation
                .arguments
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidArguments("missing 'query' field".into()))?;
            let max_results = invocation
                .arguments
                .get("max_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(5)
                .min(10) as usize;

            let results = backend
                .search(query, max_results)
                .await
                .map_err(|e| ToolError::Execution(e.to_string()))?;

            Ok(ToolOutput::new(json!({
                "query": query,
                "results": results.iter().map(|r| json!({
                    "title": r.title,
                    "url": r.url,
                    "snippet": r.snippet,
                })).collect::<Vec<_>>(),
                "result_count": results.len(),
            })))
        })
    }
}

// ── Utility functions ───────────────────────────────────────────────────

fn urlencoding(input: &str) -> String {
    let mut result = String::with_capacity(input.len() * 2);
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            b' ' => result.push_str("%20"),
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

fn urlencoding_decode(input: &str) -> Option<String> {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2 {
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    result.push(byte as char);
                } else {
                    return None;
                }
            } else {
                return None;
            }
        } else {
            result.push(ch);
        }
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_urlencoding() {
        let encoded = urlencoding("hello world");
        assert_eq!(encoded, "hello%20world");
    }

    #[test]
    fn test_urlencoding_decode() {
        let decoded = urlencoding_decode("hello%20world").unwrap();
        assert_eq!(decoded, "hello world");
    }

    #[test]
    fn test_parse_ddg_results() {
        let html = r#"
        <html><body>
        <div class="result">
            <a class="result__a" href="https://example.com">Example Title</a>
            <a class="result__snippet">This is a snippet about example.</a>
        </div>
        <div class="result">
            <a class="result__a" href="https://test.org">Test Page</a>
            <a class="result__snippet">A test page snippet.</a>
        </div>
        </body></html>
        "#;

        let results = DuckDuckGoSearch::parse_results(html, 5);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Example Title");
        assert_eq!(results[0].url, "https://example.com");
        assert_eq!(results[0].snippet, "This is a snippet about example.");
        assert_eq!(results[1].title, "Test Page");
    }
}
