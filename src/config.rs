use std::{
    env, fmt, fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

pub const DEFAULT_API_URL: &str = "https://api.sarvam.ai/v2/chat/completions";
pub const DEFAULT_MODEL: &str = "sarvam-105b-conversations";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    Ask,
    ReadOnly,
    Yolo,
}

impl PermissionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::ReadOnly => "read-only",
            Self::Yolo => "yolo",
        }
    }
}

impl fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PermissionMode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ask" => Ok(Self::Ask),
            "read-only" | "readonly" | "read_only" => Ok(Self::ReadOnly),
            "yolo" | "full-auto" | "auto" => Ok(Self::Yolo),
            other => bail!("invalid permission mode `{other}` (use ask, read-only or yolo)"),
        }
    }
}

/// Optional `~/.config/sarvam-code/config.toml`.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileConfig {
    #[serde(rename = "api-key")]
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub api_url: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
    pub max_steps: Option<u32>,
    pub max_tokens: Option<u32>,
    pub bash_timeout_secs: Option<u64>,
    pub max_output: Option<usize>,
    pub http_timeout_secs: Option<u64>,
    pub temperature: Option<f32>,
}

/// Values supplied on the command line (highest precedence).
#[derive(Debug, Default)]
pub struct Overrides {
    pub model: Option<String>,
    pub api_url: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
    pub max_steps: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub api_key: String,
    pub api_url: String,
    pub model: String,
    /// `None` means the field is omitted from the request entirely.
    pub effort: Option<String>,
    pub permission_mode: PermissionMode,
    pub max_steps: u32,
    pub max_tokens: u32,
    pub bash_timeout_secs: u64,
    pub max_output: usize,
    pub http_timeout_secs: u64,
    pub temperature: f32,
    pub workspace: PathBuf,
    pub state_dir: PathBuf,
    pub transcript_path: PathBuf,
}

impl Config {
    pub fn load(overrides: Overrides) -> Result<Self> {
        let workspace = env::current_dir().context("cannot determine the current directory")?;
        let workspace = fs::canonicalize(&workspace).unwrap_or(workspace);

        let state_dir = default_state_dir();
        fs::create_dir_all(&state_dir)
            .with_context(|| format!("cannot create state directory: {}", state_dir.display()))?;

        let file = load_file_config()?;
        let api_key = file
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .context("config file must contain a non-empty `api-key` field")?
            .to_string();

        let model = overrides
            .model
            .or_else(|| env_nonempty("SARVAM_MODEL"))
            .or(file.model)
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());

        let api_url = overrides
            .api_url
            .or_else(|| env_nonempty("SARVAM_API_URL"))
            .or(file.api_url)
            .unwrap_or_else(|| DEFAULT_API_URL.to_string());

        // An empty effort (e.g. SARVAM_EFFORT="") explicitly disables the field.
        let effort = overrides
            .effort
            .or_else(|| env::var("SARVAM_EFFORT").ok())
            .or(file.effort)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let mode_raw = overrides
            .permission_mode
            .or_else(|| env_nonempty("PERMISSION_MODE"))
            .or(file.permission_mode)
            .unwrap_or_else(|| "ask".to_string());
        let permission_mode = PermissionMode::from_str(&mode_raw)?;

        let max_steps = overrides
            .max_steps
            .or(env_number("MAX_STEPS")?)
            .or(file.max_steps)
            .unwrap_or(20)
            .max(1);

        let max_tokens = env_number("MAX_TOKENS")?
            .or(file.max_tokens)
            .unwrap_or(8000)
            .max(1);
        let bash_timeout_secs = env_number("BASH_TIMEOUT")?
            .or(file.bash_timeout_secs)
            .unwrap_or(60)
            .max(1);
        let max_output = env_number("MAX_OUTPUT")?
            .or(file.max_output)
            .unwrap_or(8000)
            .max(1000);
        let http_timeout_secs = env_number("HTTP_TIMEOUT")?
            .or(file.http_timeout_secs)
            .unwrap_or(300)
            .max(10);
        let temperature = env::var("SARVAM_TEMPERATURE")
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .or(file.temperature)
            .unwrap_or(0.2);

        Ok(Self {
            api_key,
            api_url,
            model,
            effort,
            permission_mode,
            max_steps,
            max_tokens,
            bash_timeout_secs,
            max_output,
            http_timeout_secs,
            temperature,
            transcript_path: state_dir.join("history.json"),
            workspace,
            state_dir,
        })
    }

    pub fn system_prompt(&self) -> String {
        format!(
            "You are sarvam-code, a minimal coding agent working in {} on {}.
You can inspect and modify files, and run shell commands, through the provided tools.
Rules:
- Inspect before you change: list a directory or read a file before editing it.
- Prefer edit_file for targeted changes; use write_file for new files or full rewrites.
- For multi-step work, briefly state a plan; keep progress updates concise and tied to useful findings or actions.
- Review the relevant code and existing changes before editing. Make the smallest coherent change, then run an appropriate focused check.
- Never run destructive commands (rm -rf, git reset --hard, force push) unless explicitly asked.
- If a tool result says it was truncated, request a narrower slice instead of guessing.
- Every write, edit, and bash command requires user permission; if denied, adapt instead of retrying blindly.
- In the final reply, summarize the result, report only checks actually run, and state anything incomplete or uncertain.
- Answer in the language the user writes in.",
            self.workspace.display(),
            env::consts::OS
        )
    }
}

fn env_nonempty(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn env_number<T>(key: &str) -> Result<Option<T>>
where
    T: FromStr,
    T::Err: fmt::Display,
{
    match env::var(key) {
        Ok(raw) if !raw.trim().is_empty() => match raw.trim().parse::<T>() {
            Ok(value) => Ok(Some(value)),
            Err(_) => bail!("{key} must be a number, got `{raw}`"),
        },
        _ => Ok(None),
    }
}

fn default_state_dir() -> PathBuf {
    if let Some(dir) = env_nonempty("SARVAM_STATE_DIR") {
        return PathBuf::from(dir);
    }
    dirs::state_dir()
        .or_else(dirs::data_dir)
        .map(|dir| dir.join("sarvam-code"))
        .unwrap_or_else(|| env::temp_dir().join("sarvam-code"))
}

fn load_file_config() -> Result<FileConfig> {
    let path = match env_nonempty("SARVAM_CONFIG") {
        Some(path) => PathBuf::from(path),
        None => match dirs::config_dir() {
            Some(dir) => dir.join("sarvam-code").join("config.toml"),
            None => return Ok(FileConfig::default()),
        },
    };
    if !path.exists() {
        return Ok(FileConfig::default());
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("cannot read config file: {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("cannot parse config file: {}", path.display()))
}

pub fn is_workspace_path(workspace: &Path, candidate: &Path) -> bool {
    candidate == workspace || candidate.starts_with(workspace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modes() {
        assert_eq!(
            PermissionMode::from_str("ask").unwrap(),
            PermissionMode::Ask
        );
        assert_eq!(
            PermissionMode::from_str("read-only").unwrap(),
            PermissionMode::ReadOnly
        );
        assert_eq!(
            PermissionMode::from_str("YOLO").unwrap(),
            PermissionMode::Yolo
        );
        assert!(PermissionMode::from_str("sometimes").is_err());
    }

    #[test]
    fn parses_hyphenated_api_key_field() {
        let config: FileConfig = toml::from_str("api-key = \"test-key\"").unwrap();
        assert_eq!(config.api_key.as_deref(), Some("test-key"));
    }
}
