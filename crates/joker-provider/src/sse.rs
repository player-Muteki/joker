//! Shared SSE tokenizer used by all provider stream parsers.
//!
//! Converts raw byte-stream chunks into discrete [`SseEvent`]s per the SSE
//! spec: blocks separated by blank lines, `event:` / `data:` fields, multi-line
//! data joined with newlines, and comment lines skipped. Provider-specific
//! state machines sit on top of this and only see complete events.

/// A single parsed SSE event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseEvent {
    /// Value of the `event:` field, if present.
    pub event_type: Option<String>,
    /// Value of the `data:` field (multi-line data joined with `\n`).
    pub data: String,
}

/// Incremental SSE tokenizer.
#[derive(Default)]
pub struct SseTokenizer {
    buffer: String,
    event_type: Option<String>,
    data: Vec<String>,
}

impl SseTokenizer {
    /// Create a new empty tokenizer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk of text; returns any complete events delimited so far.
    pub fn push(&mut self, chunk: &str) -> Vec<SseEvent> {
        self.buffer.push_str(&chunk.replace("\r\n", "\n"));
        self.drain_events()
    }

    /// Flush any trailing partial event at end of stream.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let block = std::mem::take(&mut self.buffer);
        if block.is_empty() {
            return Vec::new();
        }
        self.parse_block(&block).into_iter().collect()
    }

    fn drain_events(&mut self) -> Vec<SseEvent> {
        let mut events = Vec::new();
        while let Some(pos) = self.buffer.find("\n\n") {
            let block = self.buffer[..pos].to_string();
            self.buffer.drain(..pos + 2);
            if let Some(event) = self.parse_block(&block) {
                events.push(event);
            }
        }
        events
    }

    fn parse_block(&mut self, block: &str) -> Option<SseEvent> {
        self.event_type = None;
        self.data.clear();
        for line in block.lines() {
            if line.starts_with(':') {
                continue;
            }
            if let Some(value) = line.strip_prefix("event:") {
                self.event_type = Some(value.trim().to_string());
            } else if let Some(value) = line.strip_prefix("data:") {
                self.data.push(value.trim_start().to_string());
            }
        }
        if self.data.is_empty() {
            return None;
        }
        Some(SseEvent {
            event_type: self.event_type.clone(),
            data: self.data.join("\n"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(event_type: Option<&str>, data: &str) -> SseEvent {
        SseEvent {
            event_type: event_type.map(String::from),
            data: data.into(),
        }
    }

    #[test]
    fn parses_single_event() {
        let mut t = SseTokenizer::new();
        let events = t.push("event: message_start\ndata: {\"a\":1}\n\n");
        assert_eq!(events, vec![data(Some("message_start"), r#"{"a":1}"#)]);
    }

    #[test]
    fn parses_multiple_events_across_chunks() {
        let mut t = SseTokenizer::new();
        let events = t.push("data: one\n\ndata: two\n\n");
        assert_eq!(events, vec![data(None, "one"), data(None, "two")]);
        let events2 = t.push("data: three\n\n");
        assert_eq!(events2, vec![data(None, "three")]);
    }

    #[test]
    fn handles_crlf() {
        let mut t = SseTokenizer::new();
        let events = t.push("data: hello\r\n\r\n");
        assert_eq!(events, vec![data(None, "hello")]);
    }

    #[test]
    fn joins_multi_line_data() {
        let mut t = SseTokenizer::new();
        let events = t.push("data: line1\ndata: line2\n\n");
        assert_eq!(events, vec![data(None, "line1\nline2")]);
    }

    #[test]
    fn skips_comments_and_other_fields() {
        let mut t = SseTokenizer::new();
        let events = t.push(": keep-alive\nid: 42\ndata: payload\n\n");
        assert_eq!(events, vec![data(None, "payload")]);
    }

    #[test]
    fn splits_event_across_chunk_boundary() {
        let mut t = SseTokenizer::new();
        assert!(t.push("data: hel").is_empty());
        let events = t.push("lo\n\ndata: next\n\n");
        assert_eq!(events, vec![data(None, "hello"), data(None, "next")]);
    }

    #[test]
    fn finish_flushes_trailing_event() {
        let mut t = SseTokenizer::new();
        assert!(t.push("data: tail").is_empty());
        assert_eq!(t.finish(), vec![data(None, "tail")]);
    }

    #[test]
    fn skips_block_without_data_line() {
        let mut t = SseTokenizer::new();
        let events = t.push("event: ping\n\n");
        assert!(events.is_empty());
    }

    #[test]
    fn handles_blank_lines_between_events() {
        let mut t = SseTokenizer::new();
        let events = t.push("data: a\n\n\n\ndata: b\n\n");
        assert_eq!(events, vec![data(None, "a"), data(None, "b")]);
    }
}
