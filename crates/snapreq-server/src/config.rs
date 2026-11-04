//! Server configuration: defaults, a TOML file, `SNAPREQ_*` environment variables and
//! command-line values, in increasing precedence.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::outbound::{OutboundConfig, OutboundPolicy};
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
    /// Collections served by the API, one directory each.
    pub workspace: PathBuf,
    /// Allow a non-loopback address without accounts (a trusted network). Off by default:
    /// without a login, a reachable server is an open proxy into the network it runs in.
    pub allow_remote: bool,
    /// How long and how much history and run results to keep.
    pub retention: Retention,
    /// Which destinations executed requests may reach.
    pub outbound: OutboundPolicy,
    /// Who may use the server.
    pub auth: AuthConfig,
    /// Login through an OpenID Connect provider (`[oidc]`), with accounts on.
    pub oidc: Option<OidcConfig>,
}

/// `[oidc]`: single sign-on through an OpenID Connect provider (Keycloak, Authentik, Google …).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OidcConfig {
    /// Issuer URL; `<issuer>/.well-known/openid-configuration` must describe it.
    pub issuer: String,
    pub client_id: String,
    /// Also from `SNAPREQ_OIDC_CLIENT_SECRET` (which wins).
    #[serde(default)]
    pub client_secret: Option<String>,
    /// This server's callback as the provider knows it:
    /// `https://snapreq.example.com/api/v1/auth/oidc/callback`.
    pub redirect_url: String,
    /// Label of the login button ("Log in with …").
    #[serde(default = "default_oidc_name")]
    pub name: String,
    #[serde(default = "default_scopes")]
    pub scopes: Vec<String>,
    /// If set, only verified e-mail addresses of these domains may log in.
    #[serde(default)]
    pub allowed_domains: Vec<String>,
}

fn default_oidc_name() -> String {
    "single sign-on".into()
}

fn default_scopes() -> Vec<String> {
    ["openid", "email", "profile"].map(String::from).to_vec()
}

impl std::fmt::Debug for OidcConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcConfig")
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "<redacted>"),
            )
            .field("redirect_url", &self.redirect_url)
            .field("allowed_domains", &self.allowed_domains)
            .finish_non_exhaustive()
    }
}

/// Whether the server has accounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    /// No login: everyone who can reach the server is its single user. Only allowed on a
    /// loopback address, unless `allow_remote` says the network is trusted.
    None,
    /// Local accounts: the API needs a session cookie or an API token.
    Accounts,
}

impl AuthMode {
    fn parse(text: &str) -> Result<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(Self::None),
            "accounts" => Ok(Self::Accounts),
            other => Err(Error::Config(format!(
                "invalid auth mode `{other}` (expected `none` or `accounts`)"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Accounts => "accounts",
        }
    }
}

/// Authentication settings (`[auth]` in the config file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthConfig {
    pub mode: AuthMode,
    /// How long a login lasts.
    pub session_ttl: std::time::Duration,
    /// Mark the session cookie `Secure` (set it when users reach the server over HTTPS,
    /// e.g. behind a TLS-terminating proxy).
    pub secure_cookies: bool,
    /// Without accounts on a loopback address, host names besides `localhost` and loopback
    /// addresses that the server answers to (DNS-rebinding protection).
    pub allowed_hosts: Vec<String>,
}

/// Default session length in days.
pub const DEFAULT_SESSION_DAYS: u64 = 14;

impl Default for AuthConfig {
    /// No login (a loopback server), default session length, cookies without `Secure`.
    fn default() -> Self {
        Self {
            mode: AuthMode::None,
            session_ttl: std::time::Duration::from_secs(DEFAULT_SESSION_DAYS * 24 * 3600),
            secure_cookies: false,
            allowed_hosts: Vec::new(),
        }
    }
}

/// `[auth]` in the config file.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthFile {
    mode: Option<AuthMode>,
    session_days: Option<u64>,
    secure_cookies: Option<bool>,
    #[serde(default)]
    allowed_hosts: Vec<String>,
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
    workspace: Option<PathBuf>,
    allow_remote: Option<bool>,
    #[serde(default)]
    history: HistoryConfig,
    #[serde(default)]
    outbound: OutboundConfig,
    #[serde(default)]
    auth: AuthFile,
    oidc: Option<OidcConfig>,
}

/// Values given on the command line; `None` means "not given".
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub bind: Option<String>,
    pub data_dir: Option<PathBuf>,
    pub workspace: Option<PathBuf>,
    pub config_file: Option<PathBuf>,
    pub allow_remote: bool,
    /// `none` or `accounts`.
    pub auth: Option<String>,
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
        let workspace = overrides
            .workspace
            .clone()
            .or_else(|| env("SNAPREQ_WORKSPACE").map(PathBuf::from))
            .or(file.workspace.clone())
            .unwrap_or_else(|| data_dir.join("workspace"));
        let remote = !bind.ip().is_loopback();
        let mode = match overrides.auth.clone().or_else(|| env("SNAPREQ_AUTH")) {
            Some(text) => AuthMode::parse(&text)?,
            None => file.auth.mode.unwrap_or(if remote && !allow_remote {
                AuthMode::Accounts
            } else {
                AuthMode::None
            }),
        };
        let session_days = file.auth.session_days.unwrap_or(DEFAULT_SESSION_DAYS);
        if session_days == 0 || session_days > 365 {
            return Err(Error::Config(
                "auth.session_days must be between 1 and 365".into(),
            ));
        }
        let auth = AuthConfig {
            mode,
            session_ttl: std::time::Duration::from_secs(session_days * 24 * 3600),
            secure_cookies: file.auth.secure_cookies.unwrap_or(false),
            allowed_hosts: file.auth.allowed_hosts.clone(),
        };
        // Cloud metadata endpoints are blocked by default whenever others can reach the server.
        let outbound = OutboundPolicy::from_config(&file.outbound, allow_remote || remote)
            .map_err(Error::Config)?;
        let oidc = match file.oidc {
            Some(mut o) => {
                if let Some(secret) = env("SNAPREQ_OIDC_CLIENT_SECRET") {
                    o.client_secret = Some(secret);
                }
                for (key, value) in [("issuer", &o.issuer), ("redirect_url", &o.redirect_url)] {
                    let ok = url::Url::parse(value).is_ok_and(|u| {
                        matches!(u.scheme(), "http" | "https") && u.host().is_some()
                    });
                    if !ok {
                        return Err(Error::Config(format!(
                            "oidc.{key} must be an http(s) URL, not `{value}`"
                        )));
                    }
                }
                if !o.scopes.iter().any(|s| s == "openid") {
                    o.scopes.insert(0, "openid".into());
                }
                o.allowed_domains = o
                    .allowed_domains
                    .iter()
                    .map(|d| d.trim().trim_start_matches('@').to_ascii_lowercase())
                    .collect();
                if mode != AuthMode::Accounts {
                    return Err(Error::Config(
                        "[oidc] needs accounts: set auth.mode = \"accounts\" (or --auth accounts)"
                            .into(),
                    ));
                }
                Some(o)
            }
            None => None,
        };
        let config = Self {
            oidc,
            auth,
            outbound,
            bind,
            workspace,
            data_dir,
            allow_remote,
            retention,
        };
        config.check()?;
        Ok(config)
    }

    fn check(&self) -> Result<()> {
        if !self.bind.ip().is_loopback() && self.auth.mode == AuthMode::None && !self.allow_remote {
            return Err(Error::Config(format!(
                "refusing to listen on {} without accounts: anyone who can reach it could send requests from this machine. Use accounts (--auth accounts), a loopback address, or pass --allow-remote (SNAPREQ_ALLOW_REMOTE=1) if the network is trusted",
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
        assert_eq!(c.workspace, PathBuf::from("/data/workspace"));
        let w = Config::load(
            &Overrides::default(),
            env(&[("SNAPREQ_WORKSPACE", "/w")]),
            Some("/data".into()),
        )
        .unwrap();
        assert_eq!(w.workspace, PathBuf::from("/w"));
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
    fn remote_binds_need_accounts_or_permission() {
        let o = Overrides {
            bind: Some("0.0.0.0:7878".into()),
            ..Overrides::default()
        };
        // A remote bind turns accounts on by default.
        let c = Config::load(&o, env(&[]), Some("/d".into())).unwrap();
        assert_eq!(c.auth.mode, AuthMode::Accounts);
        assert!(c.outbound.block_metadata);
        // Without accounts it needs --allow-remote.
        let none = Overrides {
            auth: Some("none".into()),
            ..o.clone()
        };
        let err = Config::load(&none, env(&[]), Some("/d".into()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("refusing to listen on 0.0.0.0:7878"), "{err}");
        let trusted =
            Config::load(&o, env(&[("SNAPREQ_ALLOW_REMOTE", "1")]), Some("/d".into())).unwrap();
        assert_eq!(
            trusted.auth.mode,
            AuthMode::None,
            "allow_remote keeps no-login"
        );
        let allowed = Overrides {
            allow_remote: true,
            ..none
        };
        assert!(Config::load(&allowed, env(&[]), Some("/d".into())).is_ok());
        let v6 = Overrides {
            bind: Some("[::1]:7878".into()),
            ..Overrides::default()
        };
        assert_eq!(
            Config::load(&v6, env(&[]), Some("/d".into()))
                .unwrap()
                .auth
                .mode,
            AuthMode::None
        );
    }

    #[test]
    fn oidc_settings() {
        let dir = temp("oidc");
        std::fs::write(
            dir.join(CONFIG_FILE),
            "[auth]\nmode = \"accounts\"\n[oidc]\nissuer = \"https://idp.example.com/realms/x\"\nclient_id = \"snapreq\"\nredirect_url = \"https://snapreq.example.com/api/v1/auth/oidc/callback\"\nscopes = [\"email\"]\nallowed_domains = [\"@Example.com\"]\n",
        )
        .unwrap();
        let c = Config::load(
            &Overrides::default(),
            env(&[("SNAPREQ_OIDC_CLIENT_SECRET", "s3cret")]),
            Some(dir.clone()),
        )
        .unwrap();
        let o = c.oidc.unwrap();
        assert_eq!(o.client_secret.as_deref(), Some("s3cret"));
        assert_eq!(o.scopes, ["openid", "email"]);
        assert_eq!(o.allowed_domains, ["example.com"]);
        assert_eq!(o.name, "single sign-on");
        assert!(!format!("{o:?}").contains("s3cret"));
        // Needs accounts.
        let none = Overrides {
            auth: Some("none".into()),
            ..Overrides::default()
        };
        assert!(
            Config::load(&none, env(&[]), Some(dir.clone()))
                .unwrap_err()
                .to_string()
                .contains("[oidc] needs accounts")
        );
        std::fs::write(
            dir.join(CONFIG_FILE),
            "[auth]\nmode = \"accounts\"\n[oidc]\nissuer = \"idp\"\nclient_id = \"x\"\nredirect_url = \"https://s/cb\"\n",
        )
        .unwrap();
        assert!(
            Config::load(&Overrides::default(), env(&[]), Some(dir))
                .unwrap_err()
                .to_string()
                .contains("oidc.issuer must be an http(s) URL")
        );
    }

    #[test]
    fn auth_settings() {
        let dir = temp("auth");
        let c = Config::load(
            &Overrides::default(),
            env(&[("SNAPREQ_AUTH", "accounts")]),
            Some(dir.clone()),
        )
        .unwrap();
        assert_eq!(c.auth.mode, AuthMode::Accounts);
        assert_eq!(
            c.auth.session_ttl,
            std::time::Duration::from_secs(DEFAULT_SESSION_DAYS * 24 * 3600)
        );
        assert!(!c.auth.secure_cookies);
        std::fs::write(
            dir.join(CONFIG_FILE),
            "[auth]\nmode = \"accounts\"\nsession_days = 2\nsecure_cookies = true\n",
        )
        .unwrap();
        let c = Config::load(&Overrides::default(), env(&[]), Some(dir.clone())).unwrap();
        assert_eq!(c.auth.mode, AuthMode::Accounts);
        assert_eq!(c.auth.session_ttl.as_secs(), 2 * 24 * 3600);
        assert!(c.auth.secure_cookies);
        // The flag beats the file.
        let flag = Overrides {
            auth: Some("none".into()),
            ..Overrides::default()
        };
        assert_eq!(
            Config::load(&flag, env(&[]), Some(dir.clone()))
                .unwrap()
                .auth
                .mode,
            AuthMode::None
        );
        let bad = Overrides {
            auth: Some("ldap".into()),
            ..Overrides::default()
        };
        assert!(
            Config::load(&bad, env(&[]), Some(dir.clone()))
                .unwrap_err()
                .to_string()
                .contains("invalid auth mode `ldap`")
        );
        std::fs::write(dir.join(CONFIG_FILE), "[auth]\nsession_days = 0\n").unwrap();
        assert!(
            Config::load(&Overrides::default(), env(&[]), Some(dir))
                .unwrap_err()
                .to_string()
                .contains("session_days")
        );
    }

    #[test]
    fn outbound_policy_from_the_file() {
        let dir = temp("outbound");
        let c = Config::load(&Overrides::default(), env(&[]), Some(dir.clone())).unwrap();
        assert!(
            c.outbound.is_open(),
            "a local server restricts nothing by default"
        );
        let remote = Overrides {
            bind: Some("0.0.0.0:7878".into()),
            allow_remote: true,
            ..Overrides::default()
        };
        assert!(
            Config::load(&remote, env(&[]), Some(dir.clone()))
                .unwrap()
                .outbound
                .block_metadata
        );
        std::fs::write(
            dir.join(CONFIG_FILE),
            "[outbound]\ndeny = [\"10.0.0.0/8\"]\nblock_private = true\nblock_metadata = false\n",
        )
        .unwrap();
        let c = Config::load(&remote, env(&[]), Some(dir.clone())).unwrap();
        assert_eq!(c.outbound.deny.len(), 1);
        assert!(c.outbound.block_private && !c.outbound.block_metadata);
        std::fs::write(dir.join(CONFIG_FILE), "[outbound]\nallow = [\"x/99\"]\n").unwrap();
        assert!(
            Config::load(&Overrides::default(), env(&[]), Some(dir))
                .unwrap_err()
                .to_string()
                .contains("outbound.allow")
        );
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
