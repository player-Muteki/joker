use std::fs;
use std::path::PathBuf;
use tokio::io::AsyncReadExt;
use tracing::*;

use joker::{
    ApprovalRequirement, Tool, ToolAnnotations, ToolCapability, ToolDefinition, ToolError,
    ToolExecution, ToolFuture, ToolInvocation, ToolName, ToolOutput,
};
use serde::Deserialize;
use serde_json::json;

use crate::workspace::{parse_args, truncate_at_char_boundary, WorkspaceTool};

const TRUSTED_COMMAND_PREFIXES: &[&str] = &[
    "cargo ", "cargo test", "cargo build", "cargo check", "cargo fmt",
    "cargo clippy", "cargo doc",
    "git ", "git status", "git diff", "git log", "git show",
    "git branch", "git stash",
    "ls ", "cat ", "head ", "tail ", "echo ", "pwd", "whoami",
    "date", "which ", "type ",
    "mkdir ", "touch ",
];

const BLOCKED_ENV_VARS: &[&str] = &[
    "LD_PRELOAD", "LD_LIBRARY_PATH", "LD_AUDIT", "LD_DEBUG",
    "SHELL", "HOME", "PATH",
];

const CHAIN_SEPARATORS: &[&str] = &["&&", "||", ";", "|", "`", "$("];

#[derive(Debug, Deserialize)]
struct ShellArgs {
    command: String,
    bg: Option<bool>,
    timeout_secs: Option<u64>,
}

#[derive(Clone, Debug)]
pub(crate) struct ShellTool {
    workspace: WorkspaceTool,
}

impl ShellTool {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            workspace: WorkspaceTool::new(root),
        }
    }

    #[allow(dead_code)]
    fn is_trusted_command(command: &str) -> bool {
        let trimmed = command.trim();
        TRUSTED_COMMAND_PREFIXES.iter().any(|prefix| trimmed.starts_with(prefix))
    }

    fn analyse_command_safety(command: &str) -> Vec<String> {
        let mut warnings: Vec<String> = Vec::new();

        for var in BLOCKED_ENV_VARS {
            let pattern = format!("{var}=");
            if command.contains(&pattern) {
                warnings.push(format!("blocked environment variable assignment: {var}"));
            }
        }

        let segments = Self::split_command_chain(command);
        for segment in &segments {
            let trimmed = segment.trim();

            if trimmed.contains("..")
                && (trimmed.contains('/') || trimmed.contains('~'))
                && trimmed.contains("../../") {
                    warnings.push("path traversal detected".into());
                }

            if trimmed.contains(" &") || trimmed.ends_with('&') {
                warnings.push("background execution may cause unexpected behavior".into());
            }
        }

        if command.contains("$(") || command.contains('`') {
            warnings.push("command substitution detected".into());
        }

        warnings
    }

    fn split_command_chain(command: &str) -> Vec<String> {
        let mut segments = vec![command.to_string()];
        for sep in CHAIN_SEPARATORS {
            let mut new_segments = Vec::new();
            for seg in &segments {
                let split: Vec<&str> = seg.split(sep).collect();
                new_segments.extend(split.iter().map(|s| s.to_string()));
            }
            segments = new_segments;
        }
        segments.retain(|s| !s.trim().is_empty());
        segments
    }
}

impl Tool for ShellTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: ToolName::new("shell"),
            description: "Execute a shell command in the workspace directory.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to execute." },
                    "bg": { "type": "boolean", "description": "Run in background. Returns PID immediately." },
                    "timeout_secs": { "type": "integer", "minimum": 1, "maximum": 600, "description": "Timeout in seconds. Default: 120." }
                },
                "required": ["command"]
            }),
            annotations: ToolAnnotations::from_capabilities(
                ToolExecution::Sequential,
                vec![ToolCapability::ExecutesCode, ToolCapability::Sandboxable],
                Some(std::time::Duration::from_secs(120)),
                ApprovalRequirement::Required,
            ),
        }
    }

    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_> {
        let root = self.workspace.root.clone();
        Box::pin(async move {
            let args = parse_args::<ShellArgs>(invocation.arguments)?;
            let command_str = args.command.trim().to_string();

            if command_str.is_empty() {
                return Err(ToolError::InvalidArguments("command cannot be empty".into()));
            }

            let safety_warnings = Self::analyse_command_safety(&command_str);

            if args.bg.unwrap_or(false) {
                let child = std::process::Command::new(shell_program())
                    .arg(shell_arg())
                    .arg(&command_str)
                    .current_dir(&root)
                    .spawn()
                    .map_err(|e| ToolError::Execution(format!("bg spawn failed: {e}")))?;
                debug!(target: "tool.shell", command_preview = %command_str, pid = child.id(), "background job spawned");
                let mut res = json!({
                    "bg": true,
                    "pid": child.id(),
                    "command": command_str,
                });
                if !safety_warnings.is_empty() {
                    res["safety_warnings"] = json!(safety_warnings);
                }
                return Ok(ToolOutput::new(res));
            }

            info!(target: "tool.shell", command_preview = %command_str, "executing command");
            let mut cmd = tokio::process::Command::new(shell_program());
            set_process_group(cmd.as_std_mut());
            cmd.arg(shell_arg())
                .arg(&command_str)
                .current_dir(&root)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());

            let mut child = cmd
                .spawn()
                .map_err(|e| ToolError::Execution(format!("spawn failed: {e}")))?;

            let stdout = child.stdout.take();
            let stderr = child.stderr.take();

            let stdout_task = tokio::spawn(async move {
                let mut buf = Vec::new();
                if let Some(mut r) = stdout {
                    let _ = r.read_to_end(&mut buf).await;
                }
                buf
            });
            let stderr_task = tokio::spawn(async move {
                let mut buf = Vec::new();
                if let Some(mut r) = stderr {
                    let _ = r.read_to_end(&mut buf).await;
                }
                buf
            });

            let timeout_dur = args
                .timeout_secs
                .map(std::time::Duration::from_secs)
                .unwrap_or(std::time::Duration::from_secs(120));

            let result = tokio::time::timeout(timeout_dur, child.wait()).await;

            let (status, raw_stdout, raw_stderr) = match result {
                Ok(Ok(status)) => {
                    let out = stdout_task.await.unwrap_or_default();
                    let err = stderr_task.await.unwrap_or_default();
                    (Some(status), out, err)
                }
                Ok(Err(e)) => {
                    error!(target: "tool.shell", error = %e, command_preview = %command_str, "command execution failed");
                    return Err(ToolError::Execution(format!("command failed: {e}")));
                }
                Err(_) => {
                    kill_process_tree(child.id().unwrap_or(0));
                    let _ = child.wait().await;
                    let out = stdout_task.await.unwrap_or_default();
                    let err = stderr_task.await.unwrap_or_default();
                    let (stdout_txt, stderr_txt, spill_path) =
                        build_ring_buffer(&out, &err, 64_000, 8_000);
                    let mut res = json!({
                        "stdout": stdout_txt,
                        "stderr": stderr_txt,
                        "exit_code": null,
                        "success": false,
                        "timed_out": true,
                    });
                    if let Some(spill) = spill_path {
                        res["spill_path"] = json!(spill.to_string_lossy());
                    }
                    if !safety_warnings.is_empty() {
                        res["safety_warnings"] = json!(safety_warnings);
                    }
                    return Ok(ToolOutput::new(res));
                }
            };

            let exit_code = status.as_ref().and_then(|s| s.code());
            let success = status.is_some_and(|s| s.success());

            let output_len = raw_stdout.len() + raw_stderr.len();
            debug!(target: "tool.shell", exit_code, output_len, success, "command completed");

            let (stdout_txt, stderr_txt, spill_path) =
                build_ring_buffer(&raw_stdout, &raw_stderr, 64_000, 8_000);

            let mut res = json!({
                "stdout": stdout_txt,
                "stderr": stderr_txt,
                "exit_code": exit_code,
                "success": success,
            });

            if let Some(spill) = spill_path {
                res["spill_path"] = json!(spill.to_string_lossy());
            }
            if !safety_warnings.is_empty() {
                res["safety_warnings"] = json!(safety_warnings);
            }
            Ok(ToolOutput::new(res))
        })
    }
}

fn build_ring_buffer(stdout: &[u8], stderr: &[u8], max_stdout: usize, max_stderr: usize) -> (String, String, Option<PathBuf>) {
    let stdout_str = String::from_utf8_lossy(stdout);
    let stderr_str = String::from_utf8_lossy(stderr);

    let mut spill_path = None;

    let stdout_result = if stdout_str.len() > max_stdout {
        let spill_dir = std::env::temp_dir().join("joker-shell-spill");
        let _ = fs::create_dir_all(&spill_dir);
        let path = spill_dir.join(format!("stdout-{}", std::process::id()));
        let _ = fs::write(&path, stdout_str.as_ref());

        let truncated = truncate_at_char_boundary(stdout_str.as_ref(), max_stdout).to_string();
        spill_path = Some(path);
        format!(
            "{}\n... [output truncated, {} bytes total]",
            truncated,
            stdout_str.len()
        )
    } else {
        stdout_str.to_string()
    };

    let stderr_result = if stderr_str.len() > max_stderr {
        let t = truncate_at_char_boundary(stderr_str.as_ref(), max_stderr).to_string();
        format!(
            "{}\n... [stderr truncated, {} bytes total]",
            t,
            stderr_str.len()
        )
    } else {
        stderr_str.to_string()
    };

    (stdout_result, stderr_result, spill_path)
}

fn shell_program() -> &'static str {
    if cfg!(windows) { "cmd.exe" } else { "sh" }
}

fn shell_arg() -> &'static str {
    if cfg!(windows) { "/C" } else { "-c" }
}

#[cfg(unix)]
fn set_process_group(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

#[cfg(windows)]
fn set_process_group(_cmd: &mut std::process::Command) {}

fn kill_process_tree(pid: u32) {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill")
            .arg("--")
            .arg(format!("-{pid}"))
            .status();
        let _ = std::process::Command::new("kill")
            .arg("-9")
            .arg("--")
            .arg(format!("-{pid}"))
            .status();
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .arg("/F")
            .arg("/T")
            .arg("/PID")
            .arg(pid.to_string())
            .status();
    }
}
