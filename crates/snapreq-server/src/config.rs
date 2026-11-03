//! Server configuration: defaults, a TOML file, `SNAPREQ_*` environment variables and
//! command-line values, in increasing precedence.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::store::Retention;
use crate::{Error, Result};

/// Default address: loopback only.
pub const DEFAULT_BIND: &str = "127.0.0.1:7878";

/// Name of the config file looked up in the data directory.
pub const CONFIG_FILE: &str = "config.toml";

/// The effective configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub bind: SocketAddr,
    /// History, run results and other server state live here.
    pub data_dir: PathBuf,
    /// Allow binding to a non-loopback address. Off by default: until authentication
    /// and the outbound request policy exist, a reachable server is an open proxy into
    /// the network it runs in.
    pub allow_remote: bool,
    /// How long and how much history and run results to keep.
    pub retention: Retention,
}

/// `[history]` in the config file; `0` means no limit.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryConfig {
    max_age_days: Option<u64>,
    max_mb: Option<u64>,
}

/// What a config file may set (every key optional).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    bind: Option<String>,
    data_dir: Option<PathBuf>,
    allow_remote: Option<bool>,
    #[serde(default)]
    history: HistoryConfig,
}

/// Values given on the command line; `None` means "not given".
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub bind: Option<String>,
    pub data_dir: Option<PathBuf>,
    pub config_file: Option<PathBuf>,
    pub allow_remote: bool,
}

impl Config {
    /// Resolves the configuration. `env` looks up environment variables (passed in so
    /// tests stay hermetic); `default_data_dir` is the platform data directory.
    ///
    /// The config file is `overrides.config_file`, else `SNAPREQ_CONFIG`, else
    /// `<data dir>/config.toml` if it exists. Its `data_dir` key only applies when the
    /// data directory was not chosen otherwise.
    pub fn load(
        overrides: &Overrides,
        env: impl Fn(&str) -> Option<String>,
        default_data_dir: Option<PathBuf>,
    ) -> Result<Self> {
        let env = |k: &str| env(k).filter(|v| !v.trim().is_empty());
        let explicit_dir = overrides
            .data_dir
            .clone()
            .or_else(|| env("SNAPREQ_DATA_DIR").map(PathBuf::from));

        let explicit_file = overrides
            .config_file
            .clone()
            .or_else(|| env("SNAPREQ_CONFIG").map(PathBuf::from));
        let candidate = explicit_file.clone().or_else(|| {
            explicit_dir
                .clone()
                .or_else(|| default_data_dir.clone())
                .map(|d| d.join(CONFIG_FILE))
        });
        let file = match &candidate {
            Some(path) if explicit_file.is_some() || path.is_file() => read_file(path)?,
            _ => FileConfig::default(),
        };

        let data_dir = explicit_dir
            .or(file.data_dir.clone())
            .or(default_data_dir)
            .ok_or_else(|| {
                Error::Config("no data directory: set --data-dir or SNAPREQ_DATA_DIR".into())
            })?;

        let bind_text = overrides
            .bind
            .clone()
            .or_else(|| env("SNAPREQ_BIND"))
            .or(file.bind.clone())
            .unwrap_or_else(|| DEFAULT_BIND.to_owned());
        let bind: SocketAddr = bind_text.trim().parse().map_err(|_| {
            Error::Config(format!(
                "invalid bind address `{bind_text}` (expected IP:PORT, e.g. 127.0.0.1:7878)"
            ))
        })?;

        let allow_remote = overrides.allow_remote
            || env("SNAPREQ_ALLOW_REMOTE")
                .is_some_and(|v| matches!(v.trim(), "1" | "true" | "yes"))
            || file.allow_remote.unwrap_or(false);

        let defaults = Retention::default();
        let limit = |v: Option<u64>, scale: u64, default: Option<u64>| match v {
            Some(0) => None,
            Some(n) => Some(n.saturating_mul(scale)),
            None => default,
        };
        let retention = Retention {
            max_age: limit(
                file.history.max_age_days,
                24 * 3600,
                defaults.max_age.map(|d| d.as_secs()),
            )
            .map(std::time::Duration::from_secs),
            max_bytes: limit(file.history.max_mb, 1024 * 1024, defaults.max_bytes),
        };
        let config = Self {
            bind,
            data_dir,
            allow_remote,
            retention,
        };
        config.check()?;
        Ok(config)
    }

    fn check(&self) -> Result<()> {
        if !self.bind.ip().is_loopback() && !self.allow_remote {
            return Err(Error::Config(format!(
                "refusing to listen on {}: the server has no authentication yet, so anyone who can reach it could send requests from this machine. Use a loopback address, or pass --allow-remote (SNAPREQ_ALLOW_REMOTE=1) if the network is trusted",
                self.bind
            )));
        }
        Ok(())
    }
}

fn read_file(path: &Path) -> Result<FileConfig> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| Error::Config(format!("cannot read `{}`: {e}", path.display())))?;
    toml::from_str(&text)
        .map_err(|e| Error::Config(format!("`{}`: {}", path.display(), e.message())))
}

/// The platform data directory for SnapRequest, if one can be determined:
/// `$XDG_DATA_HOME/snapreq` or `~/.local/share/snapreq` on Linux,
/// `~/Library/Application Support/snapreq` on macOS, `%APPDATA%\snapreq` on Windows.
pub fn default_data_dir(env: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let var = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = if cfg!(windows) {
        var("APPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library/Application Support"))
    } else {
        var("XDG_DATA_HOME").or_else(|| var("HOME").map(|h| h.join(".local/share")))
    }?;
    Some(base.join("snapreq"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |k| pairs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
    }

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("snapreq-config-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn defaults() {
        let c = Config::load(&Overrides::default(), env(&[]), Some("/data".into())).unwrap();
        assert_eq!(c.bind, DEFAULT_BIND.parse().unwrap());
        assert_eq!(c.data_dir, PathBuf::from("/data"));
        assert!(!c.allow_remote);
        assert!(
            Config::load(&Overrides::default(), env(&[]), None)
                .unwrap_err()
                .to_string()
                .contains("no data directory")
        );
    }

    #[test]
    fn precedence_flags_env_file_defaults() {
        let dir = temp("prec");
        std::fs::write(dir.join(CONFIG_FILE), "bind = \"127.0.0.1:9000\"\n").unwrap();
        let from_file = Config::load(&Overrides::default(), env(&[]), Some(dir.clone())).unwrap();
        assert_eq!(from_file.bind.port(), 9000);

        let from_env = Config::load(
            &Overrides::default(),
            env(&[("SNAPREQ_BIND", "127.0.0.1:9100")]),
            Some(dir.clone()),
        )
        .unwrap();
        assert_eq!(from_env.bind.port(), 9100);

        let from_flag = Config::load(
            &Overrides {
                bind: Some("127.0.0.1:9200".into()),
                ..Overrides::default()
            },
            env(&[("SNAPREQ_BIND", "127.0.0.1:9100")]),
            Some(dir.clone()),
        )
        .unwrap();
        assert_eq!(from_flag.bind.port(), 9200);

        // The file named by SNAPREQ_CONFIG must exist and can choose the data directory.
        let other = temp("prec-file");
        std::fs::write(
            other.join("server.toml"),
            format!("data_dir = {:?}\n", other.join("state")),
        )
        .unwrap();
        let c = Config::load(
            &Overrides::default(),
            env(&[(
                "SNAPREQ_CONFIG",
                other.join("server.toml").to_str().unwrap(),
            )]),
            Some(dir),
        )
        .unwrap();
        assert_eq!(c.data_dir, other.join("state"));
        let err = Config::load(
            &Overrides {
                config_file: Some(other.join("missing.toml")),
                ..Overrides::default()
            },
            env(&[]),
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("cannot read"), "{err}");
    }

    #[test]
    fn remote_binds_need_permission() {
        let o = Overrides {
            bind: Some("0.0.0.0:7878".into()),
            ..Overrides::default()
        };
        let err = Config::load(&o, env(&[]), Some("/d".into()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("refusing to listen on 0.0.0.0:7878"), "{err}");
        assert!(Config::load(&o, env(&[("SNAPREQ_ALLOW_REMOTE", "1")]), Some("/d".into())).is_ok());
        let allowed = Overrides {
            allow_remote: true,
            ..o
        };
        assert!(Config::load(&allowed, env(&[]), Some("/d".into())).is_ok());
        let v6 = Overrides {
            bind: Some("[::1]:7878".into()),
            ..Overrides::default()
        };
        assert!(Config::load(&v6, env(&[]), Some("/d".into())).is_ok());
    }

    #[test]
    fn retention_from_the_file() {
        let dir = temp("retention");
        let c = Config::load(&Overrides::default(), env(&[]), Some(dir.clone())).unwrap();
        assert_eq!(c.retention, Retention::default());
        std::fs::write(
            dir.join(CONFIG_FILE),
            "[history]\nmax_age_days = 7\nmax_mb = 0\n",
        )
        .unwrap();
        let c = Config::load(&Overrides::default(), env(&[]), Some(dir)).unwrap();
        assert_eq!(
            c.retention.max_age,
            Some(std::time::Duration::from_secs(7 * 24 * 3600))
        );
        assert_eq!(c.retention.max_bytes, None);
    }

    #[test]
    fn bad_values() {
        let o = Overrides {
            bind: Some("localhost:7878".into()),
            ..Overrides::default()
        };
        assert!(
            Config::load(&o, env(&[]), Some("/d".into()))
                .unwrap_err()
                .to_string()
                .contains("invalid bind address")
        );
        let dir = temp("bad");
        std::fs::write(dir.join(CONFIG_FILE), "bnd = 1\n").unwrap();
        let err = Config::load(&Overrides::default(), env(&[]), Some(dir))
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown field `bnd`"), "{err}");
    }

    #[test]
    fn data_dirs() {
        if cfg!(all(unix, not(target_os = "macos"))) {
            assert_eq!(
                default_data_dir(env(&[("HOME", "/h")])),
                Some(PathBuf::from("/h/.local/share/snapreq"))
            );
            assert_eq!(
                default_data_dir(env(&[("XDG_DATA_HOME", "/x"), ("HOME", "/h")])),
                Some(PathBuf::from("/x/snapreq"))
            );
        }
        assert_eq!(default_data_dir(env(&[])), None);
    }
}
