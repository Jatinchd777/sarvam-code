use std::{
    ffi::OsString,
    fmt::Write as _,
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use tokio::{process::Command, sync::watch};

/// Marker error used to distinguish user cancellation from a real tool failure.
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

pub struct ToolContext<'a> {
    pub workspace: &'a Path,
    pub state_dir: &'a Path,
    pub bash_timeout: Duration,
}

pub fn definitions() -> Value {
    json!([
      {
        "type": "function",
        "function": {
          "name": "list_dir",
          "description": "List entries in a directory inside the workspace. Use it to orient yourself before guessing paths.",
          "parameters": {
            "type": "object",
            "properties": {
              "path": { "type": "string", "description": "Directory relative to the workspace. Defaults to '.'." }
            }
          }
        }
      },
      {
        "type": "function",
        "function": {
          "name": "read_file",
          "description": "Read a text file from the workspace. Returns numbered lines. Read a file before editing it. If output is truncated, call again with a larger offset.",
          "parameters": {
            "type": "object",
            "properties": {
              "path": { "type": "string", "description": "File path relative to the workspace." },
              "offset": { "type": "integer", "description": "First line to return, 1-based. Defaults to 1." },
              "limit": { "type": "integer", "description": "Maximum lines to return. Defaults to 400." }
            },
            "required": ["path"]
          }
        }
      },
      {
        "type": "function",
        "function": {
          "name": "write_file",
          "description": "Create or overwrite a file with exact content. Use for new files or full rewrites. The user is asked for permission each time.",
          "parameters": {
            "type": "object",
            "properties": {
              "path": { "type": "string", "description": "File path relative to the workspace." },
              "content": { "type": "string", "description": "Full file content." }
            },
            "required": ["path", "content"]
          }
        }
      },
      {
        "type": "function",
        "function": {
          "name": "edit_file",
          "description": "Replace an exact string in a file. The old_string must match exactly once unless replace_all is true. The user is asked for permission each time.",
          "parameters": {
            "type": "object",
            "properties": {
              "path": { "type": "string", "description": "File path relative to the workspace." },
              "old_string": { "type": "string", "description": "Exact text to replace." },
              "new_string": { "type": "string", "description": "Replacement text." },
              "replace_all": { "type": "boolean", "description": "Replace every occurrence. Defaults to false." }
            },
            "required": ["path", "old_string", "new_string"]
          }
        }
      },
      {
        "type": "function",
        "function": {
          "name": "bash",
          "description": "Run a shell command with the workspace as the current directory. Use for builds, tests, git status and inspection. The user is asked for permission each time. Avoid destructive commands.",
          "parameters": {
            "type": "object",
            "properties": {
              "command": { "type": "string", "description": "Command line to run with bash -c." }
            },
            "required": ["command"]
          }
        }
      }
    ])
}

pub fn help_text() -> String {
    "Tools available to the model:\n\
     · list_dir   — list a directory inside the workspace\n\
     · read_file  — read a file (numbered lines, offset/limit)\n\
     · edit_file  — exact string replacement in a file\n\
     · write_file — create or overwrite a file\n\
     · bash       — run a shell command in the workspace\n\n\
     write_file, edit_file and bash always ask for approval (unless mode is yolo)."
        .to_string()
}

pub fn needs_approval(name: &str) -> bool {
    matches!(name, "write_file" | "edit_file" | "bash")
}

/// Human-readable summary and details for the permission dialog.
pub fn approval_preview(name: &str, args: &Value) -> (String, String) {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or("<missing path>");
    match name {
        "write_file" => {
            let content = args.get("content").and_then(Value::as_str).unwrap_or("");
            (
                format!("write_file → {path}"),
                format!("{} bytes\n\n{}", content.len(), content),
            )
        }
        "edit_file" => {
            let old = args.get("old_string").and_then(Value::as_str).unwrap_or("");
            let new = args.get("new_string").and_then(Value::as_str).unwrap_or("");
            (
                format!("edit_file → {path}"),
                format!("--- old\n{old}\n\n+++ new\n{new}"),
            )
        }
        "bash" => {
            let command = args.get("command").and_then(Value::as_str).unwrap_or("");
            (
                format!("bash → {command}"),
                format!("$ {command}\n\nAPI credentials are removed from the child environment."),
            )
        }
        _ => (name.to_string(), args.to_string()),
    }
}

/// Cap what the model (and the transcript) gets to see.
pub fn truncate_output(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[... truncated {} of {} bytes — request a narrower slice]",
        &s[..end],
        s.len() - end,
        s.len()
    )
}

pub async fn execute(
    name: &str,
    args: &Value,
    ctx: &ToolContext<'_>,
    cancel: &mut watch::Receiver<bool>,
) -> Result<String> {
    if *cancel.borrow() {
        return Err(anyhow::Error::new(Cancelled));
    }
    match name {
        "list_dir" => list_dir(args, ctx).await,
        "read_file" => read_file(args, ctx).await,
        "write_file" => write_file(args, ctx).await,
        "edit_file" => edit_file(args, ctx).await,
        "bash" => bash(args, ctx, cancel).await,
        other => bail!("unknown tool: {other}"),
    }
}

async fn list_dir(args: &Value, ctx: &ToolContext<'_>) -> Result<String> {
    let raw = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let path = resolve_path(ctx.workspace, raw)?;
    if !path.is_dir() {
        bail!("not a directory: {raw}");
    }
    let mut entries = Vec::new();
    let mut reader = tokio::fs::read_dir(&path)
        .await
        .with_context(|| format!("cannot list {raw}"))?;
    while let Some(entry) = reader.next_entry().await? {
        let meta = entry.metadata().await.ok();
        let file_type = entry.file_type().await.ok();
        let mut name = entry.file_name().to_string_lossy().into_owned();
        if file_type.as_ref().is_some_and(|t| t.is_dir()) {
            name.push('/');
        } else if file_type.as_ref().is_some_and(|t| t.is_symlink()) {
            name.push('@');
        }
        let size = meta.map(|m| m.len()).unwrap_or(0);
        entries.push(format!("{size:>10}  {name}"));
    }
    entries.sort();
    let total = entries.len();
    entries.truncate(200);
    let mut out = if entries.is_empty() {
        "(empty directory)".to_string()
    } else {
        entries.join("\n")
    };
    if total > 200 {
        let _ = write!(out, "\n[... {} more entries]", total - 200);
    }
    Ok(out)
}

async fn read_file(args: &Value, ctx: &ToolContext<'_>) -> Result<String> {
    let raw = args
        .get("path")
        .and_then(Value::as_str)
        .context("missing path")?;
    let path = resolve_path(ctx.workspace, raw)?;
    if !path.is_file() {
        bail!("no such file: {raw}");
    }
    let content = tokio::fs::read_to_string(&path)
        .await
        .with_context(|| format!("cannot read {raw} (is it valid UTF-8?)"))?;
    if content.is_empty() {
        return Ok("(empty file)".to_string());
    }
    let offset = args
        .get("offset")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1) as usize;
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(400)
        .clamp(1, 10_000) as usize;

    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    if offset > total {
        bail!("offset {offset} is past the end of {raw} ({total} lines)");
    }
    let start = offset - 1;
    let end = (start + limit).min(total);
    let mut out = String::new();
    for (i, line) in lines[start..end].iter().enumerate() {
        let _ = writeln!(out, "{}: {}", start + i + 1, line);
    }
    if end < total {
        let _ = writeln!(
            out,
            "[showed lines {}..{} of {} lines]",
            start + 1,
            end,
            total
        );
    }
    Ok(out)
}

async fn write_file(args: &Value, ctx: &ToolContext<'_>) -> Result<String> {
    let raw = args
        .get("path")
        .and_then(Value::as_str)
        .context("missing path")?;
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .context("missing content")?;
    let path = resolve_path(ctx.workspace, raw)?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("cannot create parent directory for {raw}"))?;
    }
    tokio::fs::write(&path, content)
        .await
        .with_context(|| format!("cannot write {raw}"))?;
    Ok(format!("ok: wrote {} bytes to {raw}", content.len()))
}

async fn edit_file(args: &Value, ctx: &ToolContext<'_>) -> Result<String> {
    let raw = args
        .get("path")
        .and_then(Value::as_str)
        .context("missing path")?;
    let old = args
        .get("old_string")
        .and_then(Value::as_str)
        .context("missing old_string")?;
    let new = args.get("new_string").and_then(Value::as_str).unwrap_or("");
    let replace_all = args
        .get("replace_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if old.is_empty() {
        bail!("old_string must not be empty");
    }
    let path = resolve_path(ctx.workspace, raw)?;
    if !path.is_file() {
        bail!("no such file: {raw}");
    }
    let content = tokio::fs::read_to_string(&path)
        .await
        .with_context(|| format!("cannot read {raw}"))?;
    let (updated, replaced) = apply_edit(&content, old, new, replace_all)?;
    tokio::fs::write(&path, updated)
        .await
        .with_context(|| format!("cannot write {raw}"))?;
    Ok(format!("ok: replaced {replaced} occurrence(s) in {raw}"))
}

fn apply_edit(content: &str, old: &str, new: &str, replace_all: bool) -> Result<(String, usize)> {
    let count = content.matches(old).count();
    if count == 0 {
        bail!("old_string was not found in the file");
    }
    if count > 1 && !replace_all {
        bail!("old_string matches {count} times; include more context or set replace_all=true");
    }
    if replace_all {
        Ok((content.replace(old, new), count))
    } else {
        Ok((content.replacen(old, new, 1), 1))
    }
}

async fn bash(
    args: &Value,
    ctx: &ToolContext<'_>,
    cancel: &mut watch::Receiver<bool>,
) -> Result<String> {
    let command = args
        .get("command")
        .and_then(Value::as_str)
        .context("missing command")?;
    if command.trim().is_empty() {
        bail!("command is empty");
    }

    let out_path = ctx.state_dir.join("last-bash.out");
    let file = std::fs::File::create(&out_path)
        .with_context(|| format!("cannot create {}", out_path.display()))?;
    let err_file = file.try_clone()?;

    // ulimit caps file writes (~1-2 MiB); credentials are stripped from the child env.
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg("ulimit -f 2048 2>/dev/null || true; exec bash -c \"$1\"")
        .arg("sarvam-code-bash")
        .arg(command)
        .current_dir(ctx.workspace)
        .env_remove("SARVAM_API_KEY")
        .env_remove("SARVAM_API_URL")
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(err_file))
        .kill_on_drop(true);

    let mut child = cmd.spawn().context("cannot start bash")?;
    let outcome = tokio::select! {
        result = tokio::time::timeout(ctx.bash_timeout, child.wait()) => {
            match result {
                Ok(Ok(status)) => Some(status),
                Ok(Err(e)) => return Err(e).context("cannot wait for bash"),
                Err(_) => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    None
                }
            }
        }
        _ = wait_cancelled(cancel) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(anyhow::Error::new(Cancelled));
        }
    };

    let bytes = std::fs::read(&out_path).unwrap_or_default();
    let mut out = String::new();
    let text = String::from_utf8_lossy(&bytes);
    let trimmed = text.trim_end_matches('\n');
    if !trimmed.is_empty() {
        out.push_str(trimmed);
        out.push('\n');
    }
    match outcome {
        None => {
            let _ = writeln!(
                out,
                "[timed out after {}s, process killed]",
                ctx.bash_timeout.as_secs()
            );
        }
        Some(status) => {
            let code = status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string());
            let _ = writeln!(out, "[exit code: {code}]");
        }
    }
    let _ = writeln!(out, "[output: {} bytes]", bytes.len());
    Ok(out)
}

async fn wait_cancelled(rx: &mut watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            return;
        }
    }
}

/// Resolve a model-supplied path, rejecting anything that escapes the workspace.
/// Works for paths that do not exist yet (needed by write_file) by canonicalizing
/// the nearest existing ancestor.
pub fn resolve_path(workspace: &Path, raw: &str) -> Result<PathBuf> {
    if raw.trim().is_empty() {
        bail!("path is empty");
    }
    let workspace = std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    let candidate = Path::new(raw);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        workspace.join(candidate)
    };
    let normalized = normalize_lexical(&joined);
    let resolved = canonicalize_ancestor(&normalized)?;
    if !crate::config::is_workspace_path(&workspace, &resolved) {
        bail!("path escapes the workspace: {raw}");
    }
    Ok(resolved)
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
}

fn canonicalize_ancestor(path: &Path) -> Result<PathBuf> {
    let mut current = path.to_path_buf();
    let mut suffix: Vec<OsString> = Vec::new();
    loop {
        match std::fs::canonicalize(&current) {
            Ok(base) => {
                let mut resolved = base;
                for part in suffix.iter().rev() {
                    resolved.push(part);
                }
                return Ok(resolved);
            }
            Err(_) => {
                let name = current
                    .file_name()
                    .ok_or_else(|| anyhow!("invalid path: {}", path.display()))?
                    .to_os_string();
                suffix.push(name);
                if !current.pop() {
                    bail!("invalid path: {}", path.display());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_sandbox_accepts_inside() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap();
        let p = resolve_path(&ws, "a/../b.txt").unwrap();
        assert!(p.starts_with(&ws));
        assert!(p.ends_with("b.txt"));
    }

    #[test]
    fn path_sandbox_rejects_escape() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::fs::canonicalize(dir.path()).unwrap();
        assert!(resolve_path(&ws, "../outside").is_err());
        assert!(resolve_path(&ws, "/etc/passwd").is_err());
        assert!(resolve_path(&ws, "a/../../outside").is_err());
    }

    #[test]
    fn edit_requires_unique_match() {
        let content = "alpha beta alpha";
        assert!(apply_edit(content, "alpha", "x", false).is_err());
        let (updated, n) = apply_edit(content, "alpha", "x", true).unwrap();
        assert_eq!(n, 2);
        assert_eq!(updated, "x beta x");
        let (updated, n) = apply_edit(content, "beta", "B", false).unwrap();
        assert_eq!(n, 1);
        assert_eq!(updated, "alpha B alpha");
    }

    #[test]
    fn truncation_keeps_char_boundaries() {
        let text = "héllo wörld";
        let out = truncate_output(text, 2);
        assert!(out.starts_with("h"));
    }
}

#[cfg(all(test, unix))]
mod symlink_tests {
    use super::*;

    #[test]
    fn path_sandbox_rejects_symlink_escape() {
        let outside = tempfile::tempdir().unwrap();
        let inside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), inside.path().join("link")).unwrap();
        let workspace = std::fs::canonicalize(inside.path()).unwrap();
        assert!(resolve_path(&workspace, "link/secret.txt").is_err());
    }
}
