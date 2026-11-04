//! `snapreq import`.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use snapreq_interop::{Format, Options};

use crate::cli::ImportArgs;
use crate::send::ArgError;

fn read(path: &Path) -> Result<String, ArgError> {
    let mut text = String::new();
    let limit = snapreq_interop::MAX_INPUT as u64 + 1;
    let result = if path == Path::new("-") {
        std::io::stdin().take(limit).read_to_string(&mut text)
    } else {
        std::fs::File::open(path).and_then(|f| f.take(limit).read_to_string(&mut text))
    };
    result.map_err(|e| ArgError(format!("cannot read `{}`: {e}", path.display())))?;
    Ok(text)
}

pub(crate) fn run(args: &ImportArgs) -> Result<(), ArgError> {
    let format = args
        .format
        .as_deref()
        .map(str::parse::<Format>)
        .transpose()
        .map_err(ArgError)?;
    let text = read(&args.file)?;
    let format = match format {
        Some(f) => f,
        None => snapreq_interop::detect(&text).map_err(|e| ArgError(e.to_string()))?,
    };
    // cURL and Hurl files carry no name of their own: use the file's.
    let stem = (args.file != Path::new("-"))
        .then(|| args.file.file_stem())
        .flatten()
        .map(|s| s.to_string_lossy().into_owned());
    let name = args.name.clone().or_else(|| {
        matches!(format, Format::Hurl | Format::Curl)
            .then(|| stem.clone())
            .flatten()
    });
    let mut imported = snapreq_interop::import(
        &text,
        &Options {
            format: Some(format),
            name,
        },
    )
    .map_err(|e| ArgError(e.to_string()))?;
    for env in &args.envs {
        let text = read(env)?;
        snapreq_interop::add_postman_environment(&mut imported, &text)
            .map_err(|e| ArgError(format!("{}: {e}", env.display())))?;
    }
    let dir = args.into.clone().unwrap_or_else(|| {
        PathBuf::from(snapreq_interop::slug(&imported.collection.name, "imported"))
    });
    // Sealed only when a key is given explicitly, as with `snapreq secret set`.
    let key = std::env::var(snapreq_collection::sealed::KEY_ENV)
        .ok()
        .filter(|k| !k.is_empty())
        .map(|k| snapreq_collection::sealed::SecretKey::decode(&k))
        .transpose()
        .map_err(|e| ArgError(format!("{}: {e}", snapreq_collection::sealed::KEY_ENV)))?;
    let summary = imported
        .write(&dir, key.as_ref())
        .map_err(|e| ArgError(e.to_string()))?;

    if args.json {
        let mut doc = serde_json::to_value(&summary).unwrap_or_default();
        doc["format"] = format.as_str().into();
        doc["dir"] = dir.display().to_string().into();
        crate::emit_json(&doc);
    }
    eprintln!(
        "imported \"{}\" ({format}) into {}: {} request{}, {} folder{}, {} flow{}",
        summary.name,
        dir.display(),
        summary.requests,
        plural(summary.requests),
        summary.folders,
        plural(summary.folders),
        summary.flows,
        plural(summary.flows),
    );
    if !summary.environments.is_empty() {
        eprintln!("environments: {}", summary.environments.join(", "));
    }
    if summary.secrets > 0 {
        eprintln!(
            "{} secret value{} stored in {} (kept out of collection files and git)",
            summary.secrets,
            plural(summary.secrets),
            dir.join(snapreq_collection::secrets::SECRETS_DIR).display()
        );
    }
    for w in &summary.warnings {
        eprintln!("warning: {w}");
    }
    Ok(())
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}
