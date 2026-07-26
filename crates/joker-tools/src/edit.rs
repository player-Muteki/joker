use std::fs;
use std::path::{Path, PathBuf};
use tracing::*;

use joker::{
    ApprovalRequirement, Tool, ToolAnnotations, ToolCapability, ToolDefinition, ToolError,
    ToolExecution, ToolFuture, ToolInvocation, ToolName, ToolOutput,
};
use serde::Deserialize;
use serde_json::json;

use crate::workspace::{parse_args, WorkspaceTool};

const SINGLE_CANDIDATE_SIMILARITY_THRESHOLD: f64 = 0.65;
const MULTIPLE_CANDIDATES_SIMILARITY_THRESHOLD: f64 = 0.65;

type Generator = Box<dyn Iterator<Item = String>>;

trait Replacer: Send + Sync {
    fn generate(&self, content: &str, find: &str) -> Generator;
}

struct SimpleReplacer;
impl Replacer for SimpleReplacer {
    fn generate(&self, _content: &str, find: &str) -> Generator {
        Box::new(std::iter::once(find.to_string()))
    }
}

struct LineTrimmedReplacer;
impl Replacer for LineTrimmedReplacer {
    fn generate(&self, content: &str, find: &str) -> Generator {
        let original_lines: Vec<&str> = content.split('\n').collect();
        let mut search_lines: Vec<&str> = find.split('\n').collect();
        if search_lines.last().map_or(false, |l| l.is_empty()) {
            search_lines.pop();
        }
        let search_len = search_lines.len();
        let mut results = Vec::new();

        for i in 0..original_lines.len().saturating_sub(search_len - 1) {
            let mut matches = true;
            for j in 0..search_len {
                if original_lines[i + j].trim() != search_lines[j].trim() {
                    matches = false;
                    break;
                }
            }
            if !matches {
                continue;
            }
            let mut start = 0usize;
            for k in 0..i {
                start += original_lines[k].len() + 1;
            }
            let mut end = start;
            for k in 0..search_len {
                end += original_lines[i + k].len();
                if k < search_len - 1 {
                    end += 1;
                }
            }
            results.push(content[start..end].to_string());
        }
        Box::new(results.into_iter())
    }
}

fn levenshtein(a: &str, b: &str) -> usize {
    if a.is_empty() || b.is_empty() {
        return a.len().max(b.len());
    }
    let a_len = a.len();
    let b_len = b.len();
    let mut matrix = vec![vec![0usize; b_len + 1]; a_len + 1];
    for i in 0..=a_len {
        matrix[i][0] = i;
    }
    for j in 0..=b_len {
        matrix[0][j] = j;
    }
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    for i in 1..=a_len {
        for j in 1..=b_len {
            let cost = if a_bytes[i - 1] == b_bytes[j - 1] { 0 } else { 1 };
            matrix[i][j] = (matrix[i - 1][j] + 1)
                .min(matrix[i][j - 1] + 1)
                .min(matrix[i - 1][j - 1] + cost);
        }
    }
    matrix[a_len][b_len]
}

struct BlockAnchorReplacer;
impl Replacer for BlockAnchorReplacer {
    fn generate(&self, content: &str, find: &str) -> Generator {
        let original_lines: Vec<&str> = content.split('\n').collect();
        let mut search_lines: Vec<&str> = find.split('\n').collect();
        if search_lines.last().map_or(false, |l| l.is_empty()) {
            search_lines.pop();
        }
        if search_lines.len() < 3 {
            return Box::new(std::iter::empty());
        }

        let first_search = search_lines[0].trim();
        let last_search = search_lines[search_lines.len() - 1].trim();
        let search_size = search_lines.len();
        let max_delta = (search_size as f64 * 0.25).ceil() as usize;

        struct Candidate { start: usize, end: usize }

        let mut candidates = Vec::new();
        for i in 0..original_lines.len() {
            if original_lines[i].trim() != first_search {
                continue;
            }
            for j in (i + 2)..original_lines.len() {
                if original_lines[j].trim() == last_search {
                    let block_size = j - i + 1;
                    if block_size.abs_diff(search_size) <= max_delta {
                        candidates.push(Candidate { start: i, end: j });
                    }
                    break;
                }
            }
        }

        if candidates.is_empty() {
            return Box::new(std::iter::empty());
        }

        let mut results = Vec::new();

        if candidates.len() == 1 {
            let c = &candidates[0];
            let block_size = c.end - c.start + 1;
            let check = search_size.min(block_size);
            let mut similarity = 0.0;
            let lines_to_check = check.saturating_sub(2);
            if lines_to_check > 0 {
                for j in 1..lines_to_check + 1 {
                    let ol = original_lines[c.start + j].trim();
                    let sl = search_lines[j].trim();
                    let max_len = ol.len().max(sl.len());
                    if max_len == 0 {
                        continue;
                    }
                    let dist = levenshtein(ol, sl);
                    similarity += (1.0 - dist as f64 / max_len as f64) / lines_to_check as f64;
                    if similarity >= SINGLE_CANDIDATE_SIMILARITY_THRESHOLD {
                        break;
                    }
                }
            } else {
                similarity = 1.0;
            }
            if similarity >= SINGLE_CANDIDATE_SIMILARITY_THRESHOLD {
                let mut start_idx = 0usize;
                for k in 0..c.start {
                    start_idx += original_lines[k].len() + 1;
                }
                let mut end_idx = start_idx;
                for k in c.start..=c.end {
                    end_idx += original_lines[k].len();
                    if k < c.end {
                        end_idx += 1;
                    }
                }
                results.push(content[start_idx..end_idx].to_string());
            }
            return Box::new(results.into_iter());
        }

        let mut best = None;
        let mut max_sim = -1.0f64;
        for c in &candidates {
            let block_size = c.end - c.start + 1;
            let check = search_size.min(block_size);
            let lines_to_check = check.saturating_sub(2);
            let mut similarity = 0.0;
            if lines_to_check > 0 {
                for j in 1..lines_to_check + 1 {
                    let ol = original_lines[c.start + j].trim();
                    let sl = search_lines[j].trim();
                    let max_len = ol.len().max(sl.len());
                    if max_len == 0 {
                        continue;
                    }
                    let dist = levenshtein(ol, sl);
                    similarity += 1.0 - dist as f64 / max_len as f64;
                }
                similarity /= lines_to_check as f64;
            } else {
                similarity = 1.0;
            }
            if similarity > max_sim {
                max_sim = similarity;
                best = Some((c.start, c.end));
            }
        }

        if max_sim >= MULTIPLE_CANDIDATES_SIMILARITY_THRESHOLD {
            if let Some((start_line, end_line)) = best {
                let mut start_idx = 0usize;
                for k in 0..start_line {
                    start_idx += original_lines[k].len() + 1;
                }
                let mut end_idx = start_idx;
                for k in start_line..=end_line {
                    end_idx += original_lines[k].len();
                    if k < end_line {
                        end_idx += 1;
                    }
                }
                results.push(content[start_idx..end_idx].to_string());
            }
        }
        Box::new(results.into_iter())
    }
}

struct WhitespaceNormalizedReplacer;
impl Replacer for WhitespaceNormalizedReplacer {
    fn generate(&self, content: &str, find: &str) -> Generator {
        let normalize = |s: &str| {
            let mut out = s.to_string();
            let mut prev_was_space = false;
            out.retain(|c| {
                if c.is_ascii_whitespace() {
                    if prev_was_space { return false; }
                    prev_was_space = true;
                    true
                } else {
                    prev_was_space = false;
                    true
                }
            });
            out.trim().to_string()
        };

        let normalized_find = normalize(find);
        let lines: Vec<&str> = content.split('\n').collect();
        let mut results = Vec::new();

        for line in &lines {
            if normalize(line) == normalized_find {
                results.push(line.to_string());
            } else if normalize(line).contains(&normalized_find) {
                let words: Vec<&str> = find.split_whitespace().collect();
                if !words.is_empty() {
                    let pattern = words
                        .iter()
                        .map(|w| regex::escape(w))
                        .collect::<Vec<_>>()
                        .join(r"\s+");
                    if let Ok(re) = regex::Regex::new(&pattern) {
                        if let Some(m) = re.find(line) {
                            results.push(m.as_str().to_string());
                        }
                    }
                }
            }
        }

        if find.contains('\n') {
            let find_lines: Vec<&str> = find.split('\n').collect();
            if find_lines.len() > 1 {
                for i in 0..lines.len().saturating_sub(find_lines.len() - 1) {
                    let block = lines[i..i + find_lines.len()].join("\n");
                    if normalize(&block) == normalized_find {
                        results.push(block);
                    }
                }
            }
        }

        Box::new(results.into_iter())
    }
}

struct IndentationFlexibleReplacer;
impl Replacer for IndentationFlexibleReplacer {
    fn generate(&self, content: &str, find: &str) -> Generator {
        let dedent = |s: &str| -> String {
            let lines: Vec<&str> = s.split('\n').collect();
            let non_empty: Vec<&&str> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
            if non_empty.is_empty() {
                return s.to_string();
            }
            let min_indent = non_empty
                .iter()
                .map(|l| l.len() - l.trim_start().len())
                .min()
                .unwrap_or(0);
            lines
                .iter()
                .map(|l| {
                    if l.trim().is_empty() {
                        l.to_string()
                    } else {
                        l[min_indent..].to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        let normalized_find = dedent(find);
        let content_lines: Vec<&str> = content.split('\n').collect();
        let find_lines: Vec<&str> = find.split('\n').collect();
        let mut results = Vec::new();

        for i in 0..content_lines.len().saturating_sub(find_lines.len() - 1) {
            let block = content_lines[i..i + find_lines.len()].join("\n");
            if dedent(&block) == normalized_find {
                results.push(block);
            }
        }

        Box::new(results.into_iter())
    }
}

struct EscapeNormalizedReplacer;
impl Replacer for EscapeNormalizedReplacer {
    fn generate(&self, content: &str, find: &str) -> Generator {
        let unescaped_find = unescape(find);
        let mut results = Vec::new();

        if content.contains(&unescaped_find) {
            results.push(unescaped_find.clone());
        }

        let lines: Vec<&str> = content.split('\n').collect();
        let find_lines: Vec<&str> = unescaped_find.split('\n').collect();
        for i in 0..lines.len().saturating_sub(find_lines.len() - 1) {
            let block = lines[i..i + find_lines.len()].join("\n");
            if unescape(&block) == unescaped_find {
                results.push(block);
            }
        }

        Box::new(results.into_iter())
    }
}

struct TrimmedBoundaryReplacer;
impl Replacer for TrimmedBoundaryReplacer {
    fn generate(&self, content: &str, find: &str) -> Generator {
        let trimmed = find.trim();
        if trimmed.len() == find.len() {
            return Box::new(std::iter::empty());
        }

        let mut results = Vec::new();
        if content.contains(trimmed) {
            results.push(trimmed.to_string());
        }

        let lines: Vec<&str> = content.split('\n').collect();
        let find_lines: Vec<&str> = find.split('\n').collect();
        for i in 0..lines.len().saturating_sub(find_lines.len() - 1) {
            let block = lines[i..i + find_lines.len()].join("\n");
            if block.trim() == trimmed {
                results.push(block);
            }
        }

        Box::new(results.into_iter())
    }
}

struct ContextAwareReplacer;
impl Replacer for ContextAwareReplacer {
    fn generate(&self, content: &str, find: &str) -> Generator {
        let mut search_lines: Vec<&str> = find.split('\n').collect();
        if search_lines.last().map_or(false, |l| l.is_empty()) {
            search_lines.pop();
        }
        if search_lines.len() < 3 {
            return Box::new(std::iter::empty());
        }

        let first_line = search_lines[0].trim();
        let last_line = search_lines[search_lines.len() - 1].trim();
        let content_lines: Vec<&str> = content.split('\n').collect();
        let mut results = Vec::new();

        for i in 0..content_lines.len() {
            if content_lines[i].trim() != first_line {
                continue;
            }
            for j in (i + 2)..content_lines.len() {
                if content_lines[j].trim() == last_line {
                    let block = &content_lines[i..=j];
                    if block.len() == search_lines.len() {
                        let mut matching = 0usize;
                        let mut total = 0usize;
                        for k in 1..block.len() - 1 {
                            let bl = block[k].trim();
                            let sl = search_lines[k].trim();
                            if !bl.is_empty() || !sl.is_empty() {
                                total += 1;
                                if bl == sl {
                                    matching += 1;
                                }
                            }
                        }
                        if total == 0 || (matching as f64 / total as f64) >= 0.5 {
                            results.push(block.join("\n"));
                            break;
                        }
                    }
                    break;
                }
            }
        }

        Box::new(results.into_iter())
    }
}

struct MultiOccurrenceReplacer;
impl Replacer for MultiOccurrenceReplacer {
    fn generate(&self, content: &str, find: &str) -> Generator {
        let mut start = 0usize;
        let mut results = Vec::new();
        while let Some(idx) = content[start..].find(find) {
            results.push(find.to_string());
            start += idx + find.len();
        }
        Box::new(results.into_iter())
    }
}

struct ReplacerEntry {
    replacer: &'static (dyn Replacer + Sync),
    transform_new: Option<fn(&str) -> String>,
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\'') => out.push('\''),
                Some('"') => out.push('"'),
                Some('`') => out.push('`'),
                Some('\\') => out.push('\\'),
                Some(c) => { out.push('\\'); out.push(c); }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

const REPLACERS: &[ReplacerEntry] = &[
    ReplacerEntry { replacer: &SimpleReplacer, transform_new: None },
    ReplacerEntry { replacer: &LineTrimmedReplacer, transform_new: None },
    ReplacerEntry { replacer: &BlockAnchorReplacer, transform_new: None },
    ReplacerEntry { replacer: &WhitespaceNormalizedReplacer, transform_new: None },
    ReplacerEntry { replacer: &IndentationFlexibleReplacer, transform_new: None },
    ReplacerEntry { replacer: &EscapeNormalizedReplacer, transform_new: Some(unescape) },
    ReplacerEntry { replacer: &TrimmedBoundaryReplacer, transform_new: None },
    ReplacerEntry { replacer: &ContextAwareReplacer, transform_new: None },
    ReplacerEntry { replacer: &MultiOccurrenceReplacer, transform_new: None },
];

fn is_disproportionate(search: &str, old: &str) -> bool {
    let old_lines = old.split('\n').count();
    let search_lines = search.split('\n').count();
    if search_lines >= old_lines.saturating_add(3).max(old_lines * 2) {
        return true;
    }
    if old_lines == 1 {
        return false;
    }
    search.trim().len() > old.trim().len().saturating_add(500).max(old.trim().len() * 4)
}

fn replace_inner(content: &str, old_string: &str, new_string: &str, replace_all: bool) -> Result<String, String> {
    if old_string == new_string {
        return Err("No changes to apply: oldString and newString are identical.".into());
    }
    if old_string.is_empty() {
        return Err("oldString cannot be empty when editing an existing file.".into());
    }

    let mut any_match = false;

    for entry in REPLACERS {
        for search in entry.replacer.generate(content, old_string) {
            let idx = match content.find(&search) {
                Some(i) => i,
                None => continue,
            };
            any_match = true;

            if is_disproportionate(&search, old_string) {
                return Err(
                    "Refusing replacement because the matched span is much larger than oldString. \
                     Re-read the file and provide the full exact oldString for the intended replacement."
                        .into(),
                );
            }

            let effective_new = match entry.transform_new {
                Some(f) => f(new_string),
                None => new_string.to_string(),
            };

            if replace_all {
                return Ok(content.replace(&search, &effective_new));
            }

            if let Some(last) = content.rfind(&search) {
                if idx != last {
                    continue;
                }
            }

            let mut result = content[..idx].to_string();
            result.push_str(&effective_new);
            result.push_str(&content[idx + search.len()..]);
            return Ok(result);
        }
    }

    if !any_match {
        Err("Could not find oldString in the file. \
            It must match exactly, including whitespace, indentation, and line endings."
            .into())
    } else {
        Err("Found multiple matches for oldString. \
            Provide more surrounding context to make the match unique."
            .into())
    }
}

fn normalize_line_endings_lossy(text: &str, ending: &str) -> String {
    let normalized = text.replace("\r\n", "\n");
    if ending == "\r\n" {
        normalized.replace('\n', "\r\n")
    } else {
        normalized
    }
}

fn detect_line_ending(content: &str) -> &str {
    if content.contains("\r\n") { "\r\n" } else { "\n" }
}

/// Apply a text replacement in the file at `path`.
///
/// Supports fuzzy matching via multiple strategies (exact, line-trimmed,
/// block-anchor, whitespace-normalized, indentation-flexible,
/// escape-normalized, trimmed-boundary, context-aware).
pub fn edit_file(path: &Path, old_string: &str, new_string: &str, replace_all: bool) -> Result<String, ToolError> {
    let content = fs::read_to_string(path)
        .map_err(|e| ToolError::Execution(format!("read failed: {e}")))?;

    let line_ending = detect_line_ending(&content);
    let normalized_content = content.replace("\r\n", "\n");
    let normalized_old = old_string.replace("\r\n", "\n");
    let normalized_new = new_string.replace("\r\n", "\n");

    let result = replace_inner(&normalized_content, &normalized_old, &normalized_new, replace_all)
        .map_err(ToolError::InvalidArguments)?;

    let result_with_ending = normalize_line_endings_lossy(&result, line_ending);

    let current = fs::read_to_string(path)
        .map_err(|e| ToolError::Execution(format!("stale check read: {e}")))?;
    if current != content {
        warn!(target: "tool.edit_file", path = %path.display(), "file changed between read and write");
        return Err(ToolError::Execution(
            "file changed between read and write — re-read and try again".into(),
        ));
    }

    fs::write(path, &result_with_ending)
        .map_err(|e| ToolError::Execution(format!("write failed: {e}")))?;

    info!(target: "tool.edit_file", path = %path.display(), "edit applied successfully");
    Ok(result_with_ending)
}

#[derive(Debug, Deserialize)]
struct EditFileArgs {
    path: String,
    old_string: String,
    new_string: String,
    replace_all: Option<bool>,
}

#[derive(Clone, Debug)]
pub(crate) struct EditFileTool {
    workspace: WorkspaceTool,
}

impl EditFileTool {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            workspace: WorkspaceTool::new(root),
        }
    }
}

impl Tool for EditFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: ToolName::new("edit_file"),
            description:
                "Replace text in a file with intelligent fuzzy matching. \
                 Uses multiple strategies: exact match, line-trimmed comparison, \
                 block-anchor with Levenshtein similarity, whitespace-normalized, \
                 indentation-flexible, escape-normalized, trimmed-boundary, and \
                 context-aware matching. Use replace_all=true to replace all occurrences."
                    .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative file path." },
                    "old_string": { "type": "string", "description": "The text to replace." },
                    "new_string": { "type": "string", "description": "The replacement text." },
                    "replace_all": { "type": "boolean", "description": "When true, replace all occurrences. Default: false." }
                },
                "required": ["path", "old_string", "new_string"]
            }),
            annotations: ToolAnnotations {
                execution: ToolExecution::Sequential,
                mutating: true,
                timeout: None,
                capabilities: vec![ToolCapability::WritesFiles],
                default_approval: ApprovalRequirement::Required,
            },
        }
    }

    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_> {
        Box::pin(async move {
            let args = parse_args::<EditFileArgs>(invocation.arguments)?;
            let path = self.workspace.resolve_read(&args.path)?;
            let replace_all = args.replace_all.unwrap_or(false);

            edit_file(&path, &args.old_string, &args.new_string, replace_all)?;

            Ok(ToolOutput::new(json!({
                "path": args.path,
                "replaced": true,
                "replace_all": replace_all,
            })))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_exact_match() {
        let result = replace_inner("hello world", "hello", "hi", false).unwrap();
        assert_eq!(result, "hi world");
    }

    #[test]
    fn replace_all() {
        let result = replace_inner("a a a", "a", "b", true).unwrap();
        assert_eq!(result, "b b b");
    }

    #[test]
    fn line_trimmed_match() {
        let result = replace_inner("hello\n  world\nfoo", "  world", "earth", false).unwrap();
        assert_eq!(result, "hello\nearth\nfoo");
    }

    #[test]
    fn whitespace_normalized() {
        let result = replace_inner("hello   world", "hello world", "hi earth", false).unwrap();
        assert_eq!(result, "hi earth");
    }

    #[test]
    fn indentation_flexible() {
        let content = "fn foo() {\n    let x = 1;\n    let y = 2;\n}";
        let old = "fn foo() {\nlet x = 1;\nlet y = 2;\n}";
        let result = replace_inner(content, old, "fn bar() {\n    let x = 1;\n}", false).unwrap();
        assert_eq!(result, "fn bar() {\n    let x = 1;\n}");
    }

    #[test]
    fn escape_normalized() {
        let result = replace_inner("hello\nworld", "hello\\nworld", "hi\\nearth", false).unwrap();
        assert_eq!(result, "hi\nearth");
    }

    #[test]
    fn trimmed_boundary() {
        let result = replace_inner("  hello world  ", "hello world", "hi", false).unwrap();
        assert_eq!(result, "  hi  ");
    }

    #[test]
    fn multi_occ_fallback() {
        let result = replace_inner("a b a b a", "a b", "x", true).unwrap();
        assert_eq!(result, "x x a");
    }

    #[test]
    fn identical_strings_error() {
        let err = replace_inner("test", "same", "same", false).unwrap_err();
        assert!(err.contains("identical"));
    }

    #[test]
    fn empty_old_string_error() {
        let err = replace_inner("test", "", "new", false).unwrap_err();
        assert!(err.contains("empty"));
    }

    #[test]
    fn not_found_error() {
        let err = replace_inner("test", "nope", "new", false).unwrap_err();
        assert!(err.contains("Could not find"));
    }

    #[test]
    fn multiple_matches_no_replace_all() {
        let result = replace_inner("a a a", "a", "b", false);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("multiple matches"));
    }
}
