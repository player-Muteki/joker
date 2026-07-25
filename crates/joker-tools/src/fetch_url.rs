use joker::{
    ApprovalRequirement, Tool, ToolAnnotations, ToolCapability, ToolDefinition, ToolError,
    ToolExecution, ToolFuture, ToolInvocation, ToolName, ToolOutput,
};
use serde_json::json;

/// Maximum bytes to fetch from a URL.
const MAX_FETCH_BYTES: usize = 1_000_000;

/// SSRF-protected URL fetch tool.
pub struct FetchUrlTool;

impl FetchUrlTool {
    pub fn new() -> Self {
        Self
    }

    /// Check if an IP address is a private/reserved address (SSRF protection).
    fn is_private_ip(host: &str) -> bool {
        // Skip IP check for non-IP hosts (domain names will be resolved by reqwest)
        if let Ok(addr) = host.parse::<std::net::IpAddr>() {
            match addr {
                std::net::IpAddr::V4(v4) => {
                    v4.is_loopback()
                        || v4.is_private()
                        || v4.is_link_local()
                        || v4.is_multicast()
                        || v4.octets()[0] == 0
                        || v4.octets()[0] == 100 && (v4.octets()[1] & 0b11000000) == 0b01000000 // CGNAT
                        || v4.octets()[0] == 127
                }
                std::net::IpAddr::V6(v6) => {
                    v6.is_loopback()
                        || v6.is_unspecified()
                        || v6.is_multicast()
                        || v6.segments()[0] & 0xff00 == 0xfe00 // link-local
                }
            }
        } else {
            // Domain name — check if it resolves to a private IP
            if let Ok(addrs) = std::net::ToSocketAddrs::to_socket_addrs(&(host, 0)) {
                addrs.filter_map(|a| match a {
                    std::net::SocketAddr::V4(v4) => Some(std::net::IpAddr::V4(*v4.ip())),
                    std::net::SocketAddr::V6(v6) => Some(std::net::IpAddr::V6(*v6.ip())),
                })
                .any(|ip: std::net::IpAddr| {
                    match ip {
                        std::net::IpAddr::V4(v4) => {
                            v4.is_loopback()
                                || v4.is_private()
                                || v4.is_link_local()
                                || v4.octets()[0] == 100 && (v4.octets()[1] & 0b11000000) == 0b01000000
                        }
                        std::net::IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified(),
                    }
                })
            } else {
                false // can't resolve, let reqwest try (it will fail)
            }
        }
    }

    /// Extract readable text from HTML content.
    fn extract_text(html: &str) -> String {
        let mut text = String::new();
        let mut in_tag = false;
        let mut in_script = false;
        let mut in_style = false;
        let mut tag_name = String::new();

        for ch in html.chars() {
            if ch == '<' {
                in_tag = true;
                tag_name.clear();
                continue;
            }
            if ch == '>' {
                in_tag = false;
                let tn = tag_name.trim_start_matches('/').to_lowercase();
                if tn == "script" {
                    in_script = !tag_name.starts_with('/');
                } else if tn == "style" {
                    in_style = !tag_name.starts_with('/');
                }
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push(' ');
                }
                continue;
            }
            if in_tag {
                if tag_name.len() < 10 || ch != ' ' {
                    tag_name.push(ch);
                }
                continue;
            }
            if in_script || in_style {
                continue;
            }
            text.push(ch);
        }

        // Collapse whitespace
        let mut result = String::with_capacity(text.len());
        let mut prev_space = false;
        for ch in text.chars() {
            if ch.is_whitespace() {
                if !prev_space {
                    result.push(' ');
                    prev_space = true;
                }
            } else {
                result.push(ch);
                prev_space = false;
            }
        }

        result.trim().to_string()
    }
}

impl Tool for FetchUrlTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: ToolName::new("fetch_url"),
            description: "Fetch a URL and return its text content. HTML pages are reduced to readable text.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "Absolute URL beginning with http:// or https://" }
                },
                "required": ["url"]
            }),
            annotations: ToolAnnotations {
                execution: ToolExecution::Sequential,
                mutating: false,
                timeout: Some(std::time::Duration::from_secs(30)),
                capabilities: vec![ToolCapability::Network],
                default_approval: ApprovalRequirement::Suggest,
            },
        }
    }

    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_> {
        Box::pin(async move {
            let url_str = invocation
                .arguments
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidArguments("missing 'url' field".into()))?;

            // Validate URL scheme
            let parsed = url::Url::parse(url_str)
                .map_err(|e| ToolError::InvalidArguments(format!("invalid URL: {e}")))?;

            let scheme = parsed.scheme();
            if scheme != "http" && scheme != "https" {
                return Err(ToolError::InvalidArguments(format!(
                    "unsupported scheme: {scheme} (only http/https allowed)"
                )));
            }

            // SSRF check: reject private IPs
            let host = parsed
                .host_str()
                .ok_or_else(|| ToolError::InvalidArguments("URL has no host".into()))?;
            if Self::is_private_ip(host) {
                return Err(ToolError::InvalidArguments(format!(
                    "URL resolves to a private/reserved IP: {host}"
                )));
            }

            // Fetch the URL
            let client = reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (compatible; JokerBot/1.0)")
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| ToolError::Execution(format!("failed to create HTTP client: {e}")))?;

            let response = client
                .get(url_str)
                .send()
                .await
                .map_err(|e| ToolError::Execution(format!("fetch failed: {e}")))?;

            let status = response.status().as_u16();
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("unknown")
                .to_string();

            let body = response
                .bytes()
                .await
                .map_err(|e| ToolError::Execution(format!("read failed: {e}")))?;

            let truncated = body.len() > MAX_FETCH_BYTES;
            let max_len = std::cmp::min(body.len(), MAX_FETCH_BYTES);
            let body_slice = &body[..max_len];

            // Extract text based on content type
            let is_html = content_type.contains("text/html")
                || content_type.contains("application/xhtml")
                || body_slice.starts_with(b"<!")
                || body_slice.starts_with(b"<html")
                || body_slice.starts_with(b"<HTML");

            let content = if is_html {
                let html_str = String::from_utf8_lossy(body_slice);
                Self::extract_text(&html_str)
            } else {
                String::from_utf8_lossy(body_slice).to_string()
            };

            Ok(ToolOutput::new(json!({
                "url": url_str,
                "status": status,
                "content_type": content_type,
                "content": truncate_text(&content, 64_000),
                "truncated": truncated,
            })))
        })
    }
}

fn truncate_text(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut boundary = 0;
    for (i, _) in text.char_indices() {
        if i > max_bytes {
            break;
        }
        boundary = i;
    }
    format!("{}... [truncated {} bytes]", &text[..boundary], text.len() - max_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_private_ip() {
        assert!(FetchUrlTool::is_private_ip("127.0.0.1"));
        assert!(FetchUrlTool::is_private_ip("192.168.1.1"));
        assert!(FetchUrlTool::is_private_ip("10.0.0.1"));
        assert!(FetchUrlTool::is_private_ip("172.16.0.1"));
        assert!(!FetchUrlTool::is_private_ip("8.8.8.8"));
        assert!(!FetchUrlTool::is_private_ip("93.184.216.34"));
    }

    #[test]
    fn test_extract_text_removes_html_tags() {
        let html = "<html><body><h1>Hello</h1><p>World</p></body></html>";
        let text = FetchUrlTool::extract_text(html);
        assert!(text.contains("Hello"));
        assert!(text.contains("World"));
        assert!(!text.contains("<h1>"));
    }

    #[test]
    fn test_extract_text_removes_scripts() {
        let html = "<html><script>alert('xss')</script><body>Content</body></html>";
        let text = FetchUrlTool::extract_text(html);
        assert!(!text.contains("alert"));
        assert!(text.contains("Content"));
    }
}
