//! `snapreq user`: the server's local accounts (PLAN P8.1).

use std::io::{BufRead, IsTerminal};

use snapreq_server::store::{DB_FILE, Store};

use snapreq_server::security::Audit;

use crate::cli::{AuditArgs, BackupArgs, RestoreArgs, ServerDirs, UserArgs, UserCommand};
use crate::exit;

/// Runs a `snapreq user` command; returns the exit code.
pub(crate) async fn run(args: &UserArgs) -> u8 {
    match run_inner(args).await {
        Ok(()) => exit::OK,
        Err(e) => {
            eprintln!("error: {e}");
            exit::INVALID_REQUEST
        }
    }
}

/// `snapreq audit`.
pub(crate) async fn audit(args: &AuditArgs) -> u8 {
    let result = async {
        let (store, _) = open(args.data_dir.clone(), args.config.clone()).await?;
        for e in store
            .audit_log(None, None, args.limit)
            .await?
            .into_iter()
            .rev()
        {
            let at = time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(e.at) * 1_000_000)
                .ok()
                .and_then(|t| {
                    t.format(&time::format_description::well_known::Rfc3339)
                        .ok()
                })
                .unwrap_or_else(|| e.at.to_string());
            let mut line = format!(
                "{at}  {:<26} {}",
                e.action,
                e.username.as_deref().unwrap_or("-")
            );
            if let Some(ws) = &e.workspace {
                line.push_str(&format!("  workspace={ws}"));
            }
            if let Some(t) = &e.target {
                line.push_str(&format!("  {t}"));
            }
            if let Some(ip) = &e.ip {
                line.push_str(&format!("  from {ip}"));
            }
            println!("{line}");
        }
        anyhow::Ok(())
    }
    .await;
    match result {
        Ok(()) => exit::OK,
        Err(e) => {
            eprintln!("error: {e}");
            exit::INVALID_REQUEST
        }
    }
}

fn report(result: anyhow::Result<()>) -> u8 {
    match result {
        Ok(()) => exit::OK,
        Err(e) => {
            eprintln!("error: {e}");
            exit::INVALID_REQUEST
        }
    }
}

fn server_config(dirs: &ServerDirs) -> anyhow::Result<snapreq_server::Config> {
    let env = |k: &str| std::env::var(k).ok();
    let overrides = snapreq_server::Overrides {
        data_dir: dirs.data_dir.clone(),
        workspace: dirs.workspace.clone(),
        config_file: dirs.config.clone(),
        ..Default::default()
    };
    Ok(snapreq_server::Config::load(
        &overrides,
        env,
        snapreq_server::config::default_data_dir(env),
    )?)
}

/// `snapreq backup`.
pub(crate) async fn backup(args: &BackupArgs) -> u8 {
    report(
        async {
            let config = server_config(&args.dirs)?;
            let db = config.data_dir.join(DB_FILE);
            if !db.is_file() {
                anyhow::bail!("no server data in `{}`", config.data_dir.display());
            }
            let store = Store::open(&db).await?;
            let manifest = if args.file.as_os_str() == "-" {
                let out = std::io::BufWriter::new(std::io::stdout().lock());
                snapreq_server::backup::create(&config, &store, out, args.with_key).await?
            } else {
                // Written aside and renamed, so a failed backup never looks complete.
                let tmp = args.file.with_extension("partial");
                let file = std::fs::File::create(&tmp)
                    .map_err(|e| anyhow::anyhow!("cannot write `{}`: {e}", tmp.display()))?;
                let made = snapreq_server::backup::create(
                    &config,
                    &store,
                    std::io::BufWriter::new(file),
                    args.with_key,
                )
                .await;
                match made {
                    Ok(m) => {
                        std::fs::rename(&tmp, &args.file)?;
                        m
                    }
                    Err(e) => {
                        let _ = std::fs::remove_file(&tmp);
                        return Err(e.into());
                    }
                }
            };
            store
                .audit(cli_event("server.backup", &args.file.to_string_lossy()))
                .await;
            eprintln!(
                "backed up the database (schema {}) and {} workspace files{}",
                manifest.schema,
                manifest.workspace_files,
                if manifest.includes_key {
                    ", with the secret key: keep this file safe"
                } else {
                    "; the secret key is not included: keep it (or SNAPREQ_SECRET_KEY) elsewhere"
                }
            );
            Ok(())
        }
        .await,
    )
}

/// `snapreq restore`.
pub(crate) fn restore(args: &RestoreArgs) -> u8 {
    report((|| {
        let config = server_config(&args.dirs)?;
        let manifest = if args.file.as_os_str() == "-" {
            snapreq_server::backup::restore(&config, std::io::stdin().lock(), args.force)?
        } else {
            let file = std::fs::File::open(&args.file)
                .map_err(|e| anyhow::anyhow!("cannot read `{}`: {e}", args.file.display()))?;
            snapreq_server::backup::restore(&config, std::io::BufReader::new(file), args.force)?
        };
        eprintln!(
            "restored a backup of SnapRequest {} ({} workspace files) into {}",
            manifest.snapreq,
            manifest.workspace_files,
            config.data_dir.display()
        );
        if !manifest.includes_key
            && !config
                .data_dir
                .join(snapreq_collection::sealed::KEY_FILE)
                .is_file()
            && std::env::var(snapreq_collection::sealed::KEY_ENV).is_err()
        {
            eprintln!(
                "note: the backup has no secret key; put the old key into {} or set {} before starting the server, or sealed secret values stay unset",
                config
                    .data_dir
                    .join(snapreq_collection::sealed::KEY_FILE)
                    .display(),
                snapreq_collection::sealed::KEY_ENV
            );
        }
        Ok(())
    })())
}

/// The server's database and configuration.
async fn open(
    data_dir: Option<std::path::PathBuf>,
    config_file: Option<std::path::PathBuf>,
) -> anyhow::Result<(Store, snapreq_server::Config)> {
    let env = |k: &str| std::env::var(k).ok();
    let overrides = snapreq_server::Overrides {
        data_dir,
        config_file,
        ..Default::default()
    };
    let config = snapreq_server::Config::load(
        &overrides,
        env,
        snapreq_server::config::default_data_dir(env),
    )?;
    std::fs::create_dir_all(&config.data_dir).map_err(|e| {
        anyhow::anyhow!(
            "cannot create the data directory `{}`: {e}",
            config.data_dir.display()
        )
    })?;
    let store = Store::open(&config.data_dir.join(DB_FILE)).await?;
    Ok((store, config))
}

/// An event done from the server's command line.
fn cli_event(action: &'static str, name: &str) -> Audit {
    Audit {
        username: Some("(command line)".into()),
        ..Audit::new(action).target(name.to_owned())
    }
}

async fn run_inner(args: &UserArgs) -> anyhow::Result<()> {
    let (store, config) = open(args.data_dir.clone(), args.config.clone()).await?;
    let accounts_off = config.auth.mode == snapreq_server::config::AuthMode::None;
    match &args.command {
        UserCommand::Add { name } => {
            snapreq_server::accounts::check_username(name)?;
            let password = read_password(&format!("Password for `{name}`: "))?;
            let user = store.create_user(name, &password).await?;
            eprintln!("created user `{}`", user.username);
            store.audit(cli_event("user.created", &user.username)).await;
            if accounts_off {
                eprintln!(
                    "note: this server is configured without accounts; start it with `--auth accounts` (or bind a non-loopback address) to require a login"
                );
            }
        }
        UserCommand::List => {
            for user in store.users().await? {
                println!("{}", user.username);
            }
        }
        UserCommand::Passwd { name } => {
            let password = read_password(&format!("New password for `{name}`: "))?;
            if !store.set_password(name, &password, None).await? {
                anyhow::bail!("no user `{name}`");
            }
            eprintln!("password of `{name}` changed; their sessions are ended");
            store.audit(cli_event("user.password_set", name)).await;
        }
        UserCommand::Remove { name } => {
            if !store.delete_user(name).await? {
                anyhow::bail!("no user `{name}`");
            }
            eprintln!("removed user `{name}` with their tokens, history and runs");
            store.audit(cli_event("user.removed", name)).await;
        }
    }
    Ok(())
}

/// A new password: typed twice at a hidden prompt, or the first line of stdin.
fn read_password(prompt: &str) -> anyhow::Result<String> {
    let password = if std::io::stdin().is_terminal() {
        let first = rpassword::prompt_password(prompt)?;
        let again = rpassword::prompt_password("Repeat it: ")?;
        if first != again {
            anyhow::bail!("the passwords do not match");
        }
        first
    } else {
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        line.trim_end_matches(['\r', '\n']).to_owned()
    };
    snapreq_server::accounts::check_password(&password)?;
    Ok(password)
}
