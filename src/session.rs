use std::{fs, path::Path};

use anyhow::{Context, Result};
use serde_json::Value;

/// Atomically write the full transcript (used as the agent's memory).
pub fn save_history(path: &Path, history: &[Value]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(history).context("cannot serialize the transcript")?;
    fs::write(&temporary, bytes)
        .with_context(|| format!("cannot write {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("cannot replace {}", path.display()))?;
    Ok(())
}
