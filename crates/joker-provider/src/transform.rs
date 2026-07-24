//! Message transformation utilities ported from opencode's transform.ts.
//!
//! Provides provider-specific message normalisation, tool-call ID scrubbing,
//! and reasoning content handling.

use joker::{Content, Message, Role, ToolCall, ToolResult};

/// Scrubs tool-call IDs to only contain characters safe for a given provider.
///
/// This mirrors the logic in opencode's `normalizeMessages` for Claude models:
/// all non-alphanumeric, non-underscore, non-hyphen characters are replaced
/// with `_`.
pub fn scrub_tool_call_ids<S: AsRef<str>>(messages: &[Message], provider: S) -> Vec<Message> {
    let provider = provider.as_ref();
    let scrub_char = match provider {
        "anthropic" | "bedrock" => |c: char| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' },
        "mistral" => |c: char| if c.is_alphanumeric() { c } else { '0' },
        _ => return messages.to_vec(),
    };

    messages
        .iter()
        .map(|msg| {
            let mut msg = msg.clone();
            msg.content = msg
                .content
                .into_iter()
                .map(|content| match content {
                    Content::ToolCall(ref tc) => {
                        let id: String = tc.id.chars().map(&scrub_char).collect();
                        let id = if provider == "mistral" {
                            let mut id = id;
                            id.truncate(9);
                            while id.len() < 9 {
                                id.push('0');
                            }
                            id
                        } else {
                            id
                        };
                        Content::ToolCall(ToolCall { id, ..tc.clone() })
                    }
                    Content::ToolResult(ref tr) => {
                        let call_id: String = tr.call_id.chars().map(&scrub_char).collect();
                        let call_id = if provider == "mistral" {
                            let mut call_id = call_id;
                            call_id.truncate(9);
                            while call_id.len() < 9 {
                                call_id.push('0');
                            }
                            call_id
                        } else {
                            call_id
                        };
                        Content::ToolResult(ToolResult { call_id, ..tr.clone() })
                    }
                    other => other,
                })
                .collect();
            msg
        })
        .collect()
}

/// Ensures every assistant message has a reasoning block.
///
/// DeepSeek-style models require every assistant message to include a
/// `reasoning` block; this adds an empty one if missing.
pub fn ensure_reasoning_for_model<S: AsRef<str>>(
    messages: &[Message],
    model: S,
) -> Vec<Message> {
    let model = model.as_ref().to_lowercase();
    let needs_reasoning =
        model.contains("deepseek") || model.contains("qwq") || model.contains("r1");

    if !needs_reasoning {
        return messages.to_vec();
    }

    messages
        .iter()
        .map(|msg| {
            if msg.role != Role::Assistant {
                return msg.clone();
            }
            let has_reasoning = msg.content.iter().any(|c| matches!(c, Content::Reasoning(_)));
            if has_reasoning {
                return msg.clone();
            }
            let mut content = msg.content.clone();
            content.push(Content::Reasoning(joker::ReasoningContent { text: String::new() }));
            Message { content, ..msg.clone() }
        })
        .collect()
}

/// Merge consecutive text parts into a single text content.
pub fn merge_text_parts(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .map(|msg| {
            let mut merged = Vec::with_capacity(msg.content.len());
            let mut buf = String::new();
            for part in &msg.content {
                if let Content::Text(t) = part {
                    buf.push_str(&t.text);
                } else {
                    if !buf.is_empty() {
                        merged.push(Content::text(std::mem::take(&mut buf)));
                    }
                    merged.push(part.clone());
                }
            }
            if !buf.is_empty() {
                merged.push(Content::text(buf));
            }
            Message { content: merged, ..msg.clone() }
        })
        .collect()
}

/// Filter out empty messages (Anthropic/Bedrock reject them).
pub fn filter_empty_messages(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .filter(|msg| {
            if msg.content.is_empty() {
                return false;
            }
            !msg.content.iter().all(|c| match c {
                Content::Text(t) => t.text.trim().is_empty(),
                Content::Reasoning(r) => r.text.trim().is_empty(),
                _ => false,
            })
        })
        .cloned()
        .collect()
}

/// Join system messages into a single message.
pub fn merge_system_messages(messages: &mut Vec<Message>) {
    let system_text: String = messages
        .iter()
        .filter(|m| m.role == Role::System)
        .flat_map(|m| m.content.iter())
        .filter_map(|c| {
            if let Content::Text(t) = c {
                Some(t.text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    messages.retain(|m| m.role != Role::System);

    if !system_text.is_empty() {
        messages.insert(
            0,
            Message {
                role: Role::System,
                content: vec![Content::text(system_text)],
            },
        );
    }
}

/// Full message normalisation pipeline for a given provider/model.
pub fn normalize_messages<S1: AsRef<str>, S2: AsRef<str>>(
    messages: &[Message],
    provider: S1,
    model: S2,
) -> Vec<Message> {
    let mut msgs = messages.to_vec();
    merge_system_messages(&mut msgs);
    msgs = filter_empty_messages(&msgs);
    msgs = scrub_tool_call_ids(&msgs, provider.as_ref());
    msgs = ensure_reasoning_for_model(&msgs, model.as_ref());
    msgs = merge_text_parts(&msgs);
    msgs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs_anthropic_tool_call_ids() {
        let msgs = vec![Message {
            role: Role::Assistant,
            content: vec![Content::ToolCall(ToolCall {
                id: "bad@id!".into(),
                name: "test".into(),
                arguments: serde_json::json!({}),
            })],
        }];

        let result = scrub_tool_call_ids(&msgs, "anthropic");
        assert_eq!(
            result[0].content[0],
            Content::ToolCall(ToolCall {
                id: "bad_id_".into(),
                name: "test".into(),
                arguments: serde_json::json!({}),
            })
        );
    }

    #[test]
    fn ensures_deepseek_reasoning() {
        let msgs = vec![Message {
            role: Role::Assistant,
            content: vec![Content::text("hello")],
        }];

        let result = ensure_reasoning_for_model(&msgs, "deepseek-v4");
        assert_eq!(result[0].content.len(), 2);
        assert!(matches!(result[0].content[1], Content::Reasoning(_)));
    }

    #[test]
    fn filters_empty_messages() {
        let msgs = vec![
            Message {
                role: Role::User,
                content: vec![],
            },
            Message {
                role: Role::User,
                content: vec![Content::text("hello")],
            },
        ];

        let result = filter_empty_messages(&msgs);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn merges_system_messages() {
        let mut msgs = vec![
            Message {
                role: Role::System,
                content: vec![Content::text("You are helpful.")],
            },
            Message {
                role: Role::User,
                content: vec![Content::text("hi")],
            },
        ];
        merge_system_messages(&mut msgs);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, Role::System);
    }
}
