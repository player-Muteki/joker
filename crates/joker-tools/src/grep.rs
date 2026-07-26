use std::fs;
use std::path::{Path, PathBuf};

use joker::{
    ApprovalRequirement, Tool, ToolAnnotations, ToolCapability, ToolDefinition, ToolError,
    ToolExecution, ToolFuture, ToolInvocation, ToolName, ToolOutput,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::workspace::{parse_args, WorkspaceTool};

#[derive(Debug, Deserialize)]
struct GrepArgs {
    query: String,
    path: Option<String>,
    max_matches: Option<usize>,
    context_lines: Option<usize>,
    include: Option<String>,
    exclude: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct GrepTool {
    workspace: WorkspaceTool,
}

impl GrepTool {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            workspace: WorkspaceTool::new(root),
        }
    }
}

impl Tool for GrepTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: ToolName::new("grep"),
            description: "Search workspace files for a pattern. Uses ripgrep if available, with fallback to a pure-Rust implementation.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "path": { "type": "string" },
                    "max_matches": { "type": "integer", "minimum": 1, "maximum": 200 },
                    "context_lines": { "type": "integer", "minimum": 0, "maximum": 10, "description": "Number of surrounding context lines to include before and after each match." },
                    "include": { "type": "string", "description": "Glob pattern for files to include (e.g. '*.rs')." },
                    "exclude": { "type": "string", "description": "Glob pattern for files to exclude (e.g. '*.generated.*')." }
                },
                "required": ["query"]
            }),
            annotations: ToolAnnotations {
                execution: ToolExecution::ParallelSafe,
                mutating: false,
                timeout: None,
                capabilities: vec![ToolCapability::ReadOnly],
                default_approval: ApprovalRequirement::Auto,
            },
        }
    }

    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_> {
        let root = self.workspace.root.clone();
        Box::pin(async move {
            let args = parse_args::<GrepArgs>(invocation.arguments)?;
            if args.query.is_empty() {
                return Err(ToolError::InvalidArguments("query cannot be empty".into()));
            }

            let mut result = try_ripgrep(&root, &args).await?;

            if result.as_ref().is_none_or(|v| v.is_empty()) {
                result = Some(grep_fallback(&root, &args)?);
            }

            let matches = result.unwrap_or_default();
            Ok(ToolOutput::new(json!({ "matches": matches })))
        })
    }
}

async fn try_ripgrep(root: &Path, args: &GrepArgs) -> Result<Option<Vec<Value>>, ToolError> {
    let rg_check = tokio::process::Command::new("rg")
        .arg("--version")
        .output()
        .await;
    if rg_check.is_err() {
        return Ok(None);
    }

    let mut cmd = tokio::process::Command::new("rg");
    cmd.arg("--json")
        .arg("--no-heading")
        .arg("--line-number")
        .arg("-s")
        .current_dir(root);

    if let Some(ctx) = args.context_lines {
        cmd.arg("-C").arg(ctx.to_string());
    }

    if let Some(include) = &args.include {
        cmd.arg("--glob").arg(include);
    }
    if let Some(exclude) = &args.exclude {
        cmd.arg("--glob").arg(format!("!{exclude}"));
    }

    let search_path = args.path.as_deref().unwrap_or(".");
    cmd.arg(&args.query).arg(search_path);

    let output = cmd.output().await
        .map_err(|e| ToolError::Execution(format!("rg execution: {e}")))?;

    if !output.status.success() && !output.stderr.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not found") || stderr.contains("no such") {
            return Ok(None);
        }
    }

    let max_matches = args.max_matches.unwrap_or(50).min(200);
    let mut matches: Vec<Value> = Vec::new();
    let stdout = String::from_utf8_lossy(&output.stdout);

    for line in stdout.lines() {
        if matches.len() >= max_matches {
            break;
        }
        if let Ok(parsed) = serde_json::from_str::<Value>(line) {
            let typ = parsed["type"].as_str().unwrap_or("");
            if typ == "match" {
                let data = &parsed["data"];
                let path = data["path"]["text"].as_str().unwrap_or(search_path);
                let line_num = data["line_number"].as_u64().unwrap_or(0);
                let text = data["lines"]["text"].as_str().unwrap_or("").trim();
                matches.push(json!({
                    "path": path,
                    "line": line_num,
                    "text": text,
                }));
            }
        }
    }

    Ok(Some(matches))
}

fn grep_fallback(root: &Path, args: &GrepArgs) -> Result<Vec<Value>, ToolError> {
    let max_matches = args.max_matches.unwrap_or(50).min(200);
    let context_lines = args.context_lines.unwrap_or(0);

    let include_glob = args.include.as_ref()
        .map(|p| ::glob::Pattern::new(p))
        .and_then(|r| r.ok());
    let exclude_glob = args.exclude.as_ref()
        .map(|p| ::glob::Pattern::new(p))
        .and_then(|r| r.ok());

    let start_path = if let Some(ref path) = args.path {
        root.join(path.trim_start_matches('/'))
    } else {
        root.to_path_buf()
    };

    let mut matches = Vec::new();
    grep_path_with_context(
        root,
        &start_path,
        &args.query,
        max_matches,
        context_lines,
        include_glob.as_ref(),
        exclude_glob.as_ref(),
        &mut matches,
    )?;
    Ok(matches)
}

#[allow(clippy::too_many_arguments)]
fn grep_path_with_context(
    root: &Path,
    path: &Path,
    query: &str,
    max_matches: usize,
    context_lines: usize,
    include_glob: Option<&::glob::Pattern>,
    exclude_glob: Option<&::glob::Pattern>,
    matches: &mut Vec<Value>,
) -> Result<(), ToolError> {
    if matches.len() >= max_matches {
        return Ok(());
    }
    let metadata = fs::metadata(path).map_err(|e| ToolError::Execution(e.to_string()))?;
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(|e| ToolError::Execution(e.to_string()))? {
            let entry = entry.map_err(|e| ToolError::Execution(e.to_string()))?;
            grep_path_with_context(
                root, &entry.path(), query, max_matches, context_lines,
                include_glob, exclude_glob, matches,
            )?;
            if matches.len() >= max_matches {
                break;
            }
        }
        return Ok(());
    }

    if metadata.len() > 1_000_000 {
        return Ok(());
    }

    let rel_path = path.strip_prefix(root).unwrap_or(path);
    if let Some(inc) = include_glob
        && !inc.matches_path(rel_path)
    {
        return Ok(());
    }
    if let Some(exc) = exclude_glob
        && exc.matches_path(rel_path)
    {
        return Ok(());
    }

    let Ok(content) = fs::read_to_string(path) else {
        return Ok(());
    };
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();

    for (idx, line) in lines.iter().enumerate() {
        if matches.len() >= max_matches {
            break;
        }
        if line.contains(query) {
            let ctx_start = idx.saturating_sub(context_lines);
            let ctx_end = (idx + 1 + context_lines).min(total);

            let mut context: Vec<String> = Vec::new();
            for (ci, line) in lines.iter().enumerate().take(ctx_end).skip(ctx_start) {
                let prefix = if ci == idx { ">" } else { " " };
                context.push(format!("{prefix}{line}"));
            }

            matches.push(json!({
                "path": rel_path.to_string_lossy(),
                "line": idx + 1,
                "text": line,
                "context": context,
            }));
        }
    }
    Ok(())
}
