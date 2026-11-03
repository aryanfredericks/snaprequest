//! OAuth 2.0 support for the CLI: a token cache file and opening the browser.
//!
//! Tokens are kept in `<cache dir>/snapreq/oauth-tokens.json` (readable by the owner only on
//! Unix), so later runs reuse and refresh them. The cache directory is `SNAPREQ_CACHE_DIR` if
//! set, else the platform's user cache directory.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use snapreq_core::auth::{AuthorizationPrompt, Token, TokenStore};

const FILE_NAME: &str = "oauth-tokens.json";

/// The cache directory for SnapRequest, if one can be determined.
pub(crate) fn cache_dir(env: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let var = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(dir) = var("SNAPREQ_CACHE_DIR") {
        return Some(dir);
    }
    let base = if cfg!(windows) {
        var("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library/Caches"))
    } else {
        var("XDG_CACHE_HOME").or_else(|| var("HOME").map(|h| h.join(".cache")))
    }?;
    Some(base.join("snapreq"))
}

/// The token cache file.
pub(crate) fn token_file() -> Option<PathBuf> {
    cache_dir(|k| std::env::var(k).ok()).map(|d| d.join(FILE_NAME))
}

/// A [`TokenStore`] in a JSON file. The file is read on every lookup and replaced
/// atomically on every change, so concurrent runs see each other's tokens.
#[derive(Debug)]
pub(crate) struct FileTokenStore {
    path: PathBuf,
    /// Serializes writes from this process.
    lock: Mutex<()>,
}

impl FileTokenStore {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    fn read(&self) -> BTreeMap<String, Token> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!(path = %self.path.display(), "ignoring unreadable token cache: {e}");
                BTreeMap::new()
            }),
            Err(_) => BTreeMap::new(),
        }
    }

    fn update(&self, f: impl FnOnce(&mut BTreeMap<String, Token>)) {
        let _guard = self.lock.lock();
        let mut tokens = self.read();
        f(&mut tokens);
        if let Err(e) = write_private(
            &self.path,
            &serde_json::to_vec_pretty(&tokens).unwrap_or_default(),
        ) {
            tracing::warn!(path = %self.path.display(), "cannot save the token cache: {e}");
        }
    }
}

impl TokenStore for FileTokenStore {
    fn get(&self, key: &str) -> Option<Token> {
        self.read().remove(key)
    }

    fn put(&self, key: &str, token: &Token) {
        self.update(|t| {
            t.insert(key.to_owned(), token.clone());
        });
    }

    fn remove(&self, key: &str) {
        self.update(|t| {
            t.remove(key);
        });
    }
}

/// Writes `data` to `path` through a temporary file, readable by the owner only on Unix.
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut f = options.open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// Deletes the token cache. Returns the path if a file was removed.
pub(crate) fn clear() -> Result<Option<PathBuf>, String> {
    let Some(path) = token_file() else {
        return Err("no cache directory (set SNAPREQ_CACHE_DIR or HOME)".into());
    };
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(Some(path)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot remove `{}`: {e}", path.display())),
    }
}

/// Shows the authorization URL on stderr and opens the default browser, unless
/// `SNAPREQ_NO_BROWSER` is set (for example on a remote machine).
#[derive(Debug)]
pub(crate) struct BrowserPrompt;

impl AuthorizationPrompt for BrowserPrompt {
    fn open(&self, url: &str) -> Result<(), String> {
        eprintln!("Open this URL in a browser to sign in (waiting up to 5 minutes):\n  {url}");
        if std::env::var_os("SNAPREQ_NO_BROWSER").is_some() {
            return Ok(());
        }
        let mut cmd = if cfg!(windows) {
            let mut c = std::process::Command::new("rundll32");
            c.args(["url.dll,FileProtocolHandler", url]);
            c
        } else if cfg!(target_os = "macos") {
            let mut c = std::process::Command::new("open");
            c.arg(url);
            c
        } else {
            let mut c = std::process::Command::new("xdg-open");
            c.arg(url);
            c
        };
        // Failing to open a browser is fine: the URL is printed.
        let _ = cmd
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_dir_precedence() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        assert_eq!(
            cache_dir(env(&[("SNAPREQ_CACHE_DIR", "/c"), ("HOME", "/h")])),
            Some(PathBuf::from("/c"))
        );
        if cfg!(all(unix, not(target_os = "macos"))) {
            assert_eq!(
                cache_dir(env(&[("XDG_CACHE_HOME", "/x"), ("HOME", "/h")])),
                Some(PathBuf::from("/x/snapreq"))
            );
            assert_eq!(
                cache_dir(env(&[("HOME", "/h")])),
                Some(PathBuf::from("/h/.cache/snapreq"))
            );
        }
        assert_eq!(cache_dir(env(&[])), None);
    }

    #[test]
    fn file_store_round_trip() {
        let dir = std::env::temp_dir().join(format!("snapreq-tokens-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = FileTokenStore::new(dir.join("nested").join(FILE_NAME));
        let token = Token {
            access_token: "a".into(),
            token_type: "Bearer".into(),
            expires_at: Some(10),
            refresh_token: Some("r".into()),
            scope: None,
        };
        assert!(store.get("k").is_none());
        store.put("k", &token);
        store.put("other", &token);
        // A second store on the same file (another run) sees it.
        let again = FileTokenStore::new(dir.join("nested").join(FILE_NAME));
        assert_eq!(again.get("k"), Some(token));
        again.remove("k");
        assert!(store.get("k").is_none() && store.get("other").is_some());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(dir.join("nested").join(FILE_NAME))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        std::fs::write(dir.join("nested").join(FILE_NAME), "not json").unwrap();
        assert!(store.get("other").is_none());
    }
}
