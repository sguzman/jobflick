use anyhow::{Context, Result};
use std::env;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub fn config_dir() -> PathBuf {
    base("XDG_CONFIG_HOME", ".config").join("jobflick")
}
pub fn data_dir() -> PathBuf {
    base("XDG_DATA_HOME", ".local/share").join("jobflick")
}
pub fn runtime_dir() -> PathBuf {
    match env::var_os("XDG_RUNTIME_DIR") {
        Some(value) if !value.is_empty() => PathBuf::from(value).join("jobflick"),
        _ => data_dir().join("runtime"),
    }
}
pub fn socket() -> PathBuf {
    runtime_dir().join("daemon.sock")
}

fn base(variable: &str, fallback: &str) -> PathBuf {
    if let Some(value) = env::var_os(variable) {
        if !value.is_empty() {
            return PathBuf::from(value);
        }
    }
    let home = env::var_os("HOME").unwrap_or_else(|| ".".into());
    PathBuf::from(home).join(fallback)
}

pub fn private_dir(path: &Path) -> Result<()> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .with_context(|| format!("create {}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

pub fn atomic_private_write(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path.parent().context("file without parent")?;
    private_dir(parent)?;
    let temp = parent.join(format!(".{}.tmp", Uuid::new_v4()));
    let write_result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(content)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    write_result
}
