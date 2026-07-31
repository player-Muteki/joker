//! Provider error classification.
//!
//! Maps HTTP status codes and response bodies onto the [`ModelErrorKind`]
//! taxonomy defined in `joker::model`. Retryability lives on
//! [`ModelError::is_retryable`] so both the agent loop and stream reconnection
//! share one policy.

use joker::ModelErrorKind;

/// Classify a non-success HTTP response into a [`ModelErrorKind`].
///
/// `vendor` is reserved for provider-specific quirks (e.g. OpenAI's habit of
/// returning 404 for momentarily unavailable models).
#[must_use]
pub fn classify_error(status: u16, body: &str, _vendor: &str) -> ModelErrorKind {
    let lower = body.to_lowercase();
    match status {
        401 | 403 => ModelErrorKind::Auth,
        402 => ModelErrorKind::Quota,
        429 => ModelErrorKind::RateLimited,
        400 | 413 => {
            if lower.contains("context_length_exceeded")
                || lower.contains("too long")
                || lower.contains("too large")
                || lower.contains("maximum context")
                || lower.contains("context window")
                || lower.contains("context length")
            {
                ModelErrorKind::ContextLength
            } else if lower.contains("insufficient_quota") || lower.contains("quota") {
                ModelErrorKind::Quota
            } else {
                ModelErrorKind::Unknown
            }
        }
        404 => {
            if lower.contains("model_not_found") || lower.contains("model not found") {
                ModelErrorKind::ModelNotFound
            } else {
                ModelErrorKind::Unknown
            }
        }
        408 | 500..=599 => ModelErrorKind::Network,
        _ => ModelErrorKind::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_status_codes() {
        assert_eq!(classify_error(401, "invalid key", ""), ModelErrorKind::Auth);
        assert_eq!(classify_error(403, "forbidden", ""), ModelErrorKind::Auth);
        assert_eq!(classify_error(402, "quota exceeded", ""), ModelErrorKind::Quota);
        assert_eq!(
            classify_error(429, "rate limit", ""),
            ModelErrorKind::RateLimited
        );
        assert_eq!(classify_error(500, "internal", ""), ModelErrorKind::Network);
        assert_eq!(classify_error(503, "overloaded", ""), ModelErrorKind::Network);
        assert_eq!(classify_error(408, "timeout", ""), ModelErrorKind::Network);
        assert_eq!(classify_error(200, "ok", ""), ModelErrorKind::Unknown);
        assert_eq!(classify_error(418, "teapot", ""), ModelErrorKind::Unknown);
    }

    #[test]
    fn classifies_context_length_bodies() {
        for body in [
            "{\"error\":{\"code\":\"context_length_exceeded\"}}",
            "prompt is too long: 200000 tokens",
            "this model's maximum context length is 128000",
            "exceeds the context window",
            "request exceeds the context length limit",
        ] {
            assert_eq!(
                classify_error(400, body, ""),
                ModelErrorKind::ContextLength,
                "body: {body}"
            );
        }
        assert_eq!(
            classify_error(413, "payload too large", ""),
            ModelErrorKind::ContextLength
        );
    }

    #[test]
    fn classifies_quota_bodies() {
        assert_eq!(
            classify_error(400, "{\"error\":{\"message\":\"insufficient_quota\"}}", ""),
            ModelErrorKind::Quota
        );
        assert_eq!(
            classify_error(403, "quota exceeded for project", ""),
            ModelErrorKind::Auth
        );
    }

    #[test]
    fn classifies_model_not_found() {
        assert_eq!(
            classify_error(404, "{\"error\":{\"type\":\"model_not_found\"}}", ""),
            ModelErrorKind::ModelNotFound
        );
        assert_eq!(
            classify_error(404, "model not found: gpt-5", ""),
            ModelErrorKind::ModelNotFound
        );
        assert_eq!(classify_error(404, "no route", ""), ModelErrorKind::Unknown);
    }
}
