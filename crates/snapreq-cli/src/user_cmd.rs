//! `snapreq user`: the server's local accounts (PLAN P8.1).

use std::io::{BufRead, IsTerminal};

use snapreq_server::store::{DB_FILE, Store};

use snapreq_server::security::Audit;

use crate::cli::{AuditArgs, UserArgs, UserCommand};
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
