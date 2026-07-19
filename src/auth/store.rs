//! `TokenStore` backed by a `chmod 600` plaintext file in the config dir
//! (ADR-0002). The stored blob is the yup-oauth2 token cache JSON; keeping it
//! behind the trait lets an OS-keychain backend replace it later.

use std::path::{Path, PathBuf};

use anyhow::Context;

use super::TokenStore;

/// A plaintext token file, created `0600` on unix.
pub struct FileTokenStore {
    path: PathBuf,
}

impl FileTokenStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// `<config dir>/token.json`, e.g. `~/.config/oxidone/token.json` on Linux.
    /// `None` if no platform config dir can be determined.
    pub fn in_config_dir() -> Option<Self> {
        crate::config::config_file().map(|cfg| {
            let dir = cfg.parent().map(Path::to_path_buf).unwrap_or_default();
            Self::new(dir.join("token.json"))
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl TokenStore for FileTokenStore {
    fn load(&self) -> anyhow::Result<Option<String>> {
        match std::fs::read_to_string(&self.path) {
            Ok(contents) => Ok(Some(contents)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading token file {}", self.path.display())),
        }
    }

    fn save(&self, token: &str) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating config dir {}", parent.display()))?;
        }
        write_private(&self.path, token)
            .with_context(|| format!("writing token file {}", self.path.display()))
    }

    fn clear(&self) -> anyhow::Result<()> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => {
                Err(e).with_context(|| format!("removing token file {}", self.path.display()))
            }
        }
    }
}

/// Write `contents` to `path`, ensuring the file is only readable/writable by
/// the current user (`0600`) on unix.
#[cfg(unix)]
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // `.mode()` only applies on creation; enforce 0600 even if the file
    // pre-existed with looser permissions.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(contents.as_bytes())
}

#[cfg(not(unix))]
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    std::fs::write(path, contents)
}
