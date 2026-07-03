use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::defaults::STORE_VERSION;
use crate::model::WatchStore;
use crate::util::home_dir;

pub(crate) fn state_path(state_arg: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = state_arg {
        return Ok(path);
    }
    if let Some(path) = env::var_os("COUNTERSPELL_STATE") {
        return Ok(PathBuf::from(path));
    }
    Ok(home_dir()?.join(".counterspell").join("sessions.json"))
}

pub(crate) fn load_store(path: &Path) -> Result<WatchStore> {
    if !path.exists() {
        return Ok(WatchStore::default());
    }

    let raw =
        fs::read_to_string(path).with_context(|| format!("read state file {}", path.display()))?;
    let mut store: WatchStore = serde_json::from_str(&raw)
        .with_context(|| format!("parse state file {}", path.display()))?;
    store.version = STORE_VERSION;
    Ok(store)
}

pub(crate) fn save_store(path: &Path, store: &WatchStore) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create state dir {}", parent.display()))?;
    }
    let raw = serde_json::to_vec_pretty(store).context("serialize state file")?;
    fs::write(path, raw).with_context(|| format!("write state file {}", path.display()))
}
