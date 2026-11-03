//! SQLite storage: request history and run results (ADR-006, ADR-019).
//!
//! One database file, `snapreq.db` in the data directory. Calls run on Tokio's blocking
//! pool; a single connection is shared behind a mutex (the server is the only writer).
//!
//! Nothing secret is written: entries are built with [`HistoryEntry::from_exchange`] and
//! [`RunEntry::from_report`], which hide credentials and every known secret value.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::{Value, json};
use snapreq_core::model::Header;
use snapreq_core::{DEFAULT_SENSITIVE_HEADERS, Request, Response};
use snapreq_test::Report;
use snapreq_test::report::{header_credentials, hide, hide_json, hide_url};

use crate::{Error, Result};

/// Database file name inside the data directory.
pub const DB_FILE: &str = "snapreq.db";

/// Longest response body kept in history (bytes); longer bodies are cut.
pub const MAX_STORED_BODY: usize = 1024 * 1024;

/// Schema migrations, in order. Applied once each, tracked by `PRAGMA user_version`.
const MIGRATIONS: &[&str] = &[
    // 1: history and runs
    "CREATE TABLE history (
        id INTEGER PRIMARY KEY,
        created_at INTEGER NOT NULL,
        method TEXT NOT NULL,
        url TEXT NOT NULL,
        status INTEGER,
        duration_ms REAL,
        error TEXT,
        collection TEXT,
        request_key TEXT,
        request_json TEXT NOT NULL,
        response_json TEXT,
        size_bytes INTEGER NOT NULL
    );
    CREATE INDEX history_created_at ON history (created_at);
    CREATE TABLE runs (
        id INTEGER PRIMARY KEY,
        created_at INTEGER NOT NULL,
        kind TEXT NOT NULL,
        target_key TEXT NOT NULL,
        name TEXT NOT NULL,
        passed INTEGER NOT NULL,
        report_json TEXT NOT NULL,
        size_bytes INTEGER NOT NULL
    );
    CREATE INDEX runs_created_at ON runs (created_at);",
    // 2: the JUnit XML and HTML renderings of each run's report
    "ALTER TABLE runs ADD COLUMN junit TEXT;
    ALTER TABLE runs ADD COLUMN html TEXT;",
];

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn db_err(e: rusqlite::Error) -> Error {
    Error::Storage(e.to_string())
}

/// How long and how much to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retention {
    /// Entries older than this are deleted.
    pub max_age: Option<Duration>,
    /// When history and runs together use more than this, the oldest entries go.
    pub max_bytes: Option<u64>,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            max_age: Some(Duration::from_secs(30 * 24 * 3600)),
            max_bytes: Some(512 * 1024 * 1024),
        }
    }
}

/// What a prune removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Pruned {
    pub history: usize,
    pub runs: usize,
}

/// A request and its outcome, ready to be stored (already redacted).
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    pub method: String,
    pub url: String,
    pub status: Option<u16>,
    pub duration_ms: Option<f64>,
    pub error: Option<String>,
    pub collection: Option<String>,
    pub request_key: Option<String>,
    pub request: Value,
    pub response: Option<Value>,
}

/// A row of the history list (without the large JSON columns).
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
pub struct HistorySummary {
    pub id: i64,
    /// Unix milliseconds.
    pub created_at: i64,
    pub method: String,
    pub url: String,
    pub status: Option<u16>,
    pub duration_ms: Option<f64>,
    pub error: Option<String>,
    pub collection: Option<String>,
    pub request_key: Option<String>,
}

/// One stored history entry with its request and response.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistoryRecord {
    #[serde(flatten)]
    pub summary: HistorySummary,
    pub request: Value,
    pub response: Option<Value>,
}

/// Filters for listing history (all optional; newest first).
#[derive(Debug, Clone, Default)]
pub struct HistoryQuery {
    /// Substring of the URL (case-insensitive).
    pub url_contains: Option<String>,
    pub method: Option<String>,
    pub status: Option<u16>,
    /// Status class: `2` for 200–299 and so on.
    pub status_class: Option<u16>,
    /// `true`: only requests that failed without a response; `false`: only responses.
    pub failed: Option<bool>,
    /// Only requests of this collection.
    pub collection: Option<String>,
    /// Only entries with an id below this (for paging).
    pub before_id: Option<i64>,
    pub limit: usize,
}

/// A finished run, ready to be stored.
#[derive(Debug, Clone, PartialEq)]
pub struct RunEntry {
    pub kind: String,
    pub target_key: String,
    pub name: String,
    pub passed: bool,
    pub report: Value,
    /// The report as JUnit XML and as a self-contained HTML page (`snapreq run --report`).
    pub junit: Option<String>,
    pub html: Option<String>,
}

/// A stored rendering of a run report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportFormat {
    Json,
    Junit,
    Html,
}

#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
pub struct RunSummary {
    pub id: i64,
    pub created_at: i64,
    pub kind: String,
    pub target_key: String,
    pub name: String,
    pub passed: bool,
}

/// Header values hidden in storage: credentials, cookies and API keys.
fn stored_header(h: &Header, secrets: &[String]) -> Value {
    let sensitive = DEFAULT_SENSITIVE_HEADERS
        .iter()
        .any(|s| s.eq_ignore_ascii_case(h.name.trim()));
    let value = if sensitive {
        "<redacted>".to_owned()
    } else {
        hide(&h.value, secrets)
    };
    json!({ "name": h.name, "value": value })
}

impl HistoryEntry {
    /// Builds a redacted entry: sensitive headers and every secret of the request are
    /// hidden, auth is reduced to its type, response bodies are cut at
    /// [`MAX_STORED_BODY`]. Traces are stored as the engine redacted them.
    pub fn from_exchange(
        req: &Request,
        outcome: std::result::Result<&Response, &snapreq_core::Error>,
        source: Option<(&str, &str)>,
    ) -> Self {
        let mut secrets = req.secrets();
        secrets.extend(header_credentials(req));
        let url = req
            .effective_url()
            .map_or_else(|_| req.url.clone(), |u| u.to_string());
        let url = hide_url(req, &url);
        let request = json!({
            "method": req.method.as_str(),
            "url": url,
            "headers": req.headers.iter().filter(|h| h.enabled).map(|h| stored_header(&Header::new(&h.name, &h.value), &secrets)).collect::<Vec<_>>(),
            "body": hide_json(serde_json::to_value(&req.body).unwrap_or(Value::Null), &secrets),
            "auth": req.auth.kind(),
        });
        let (status, duration_ms, error, response) = match outcome {
            Ok(resp) => {
                let mut body = resp.body.to_vec();
                let cut = body.len() > MAX_STORED_BODY;
                body.truncate(MAX_STORED_BODY);
                let body = match String::from_utf8(body) {
                    Ok(text) => {
                        json!({ "encoding": "utf8", "data": hide(&text, &secrets), "truncated": cut })
                    }
                    Err(e) => {
                        use base64::Engine as _;
                        json!({
                            "encoding": "base64",
                            "data": base64::engine::general_purpose::STANDARD.encode(e.as_bytes()),
                            "truncated": cut,
                        })
                    }
                };
                let response = json!({
                    "status": resp.status,
                    "version": resp.version,
                    "url": hide_url(req, &resp.url),
                    "headers": resp.headers.iter().map(|h| stored_header(h, &secrets)).collect::<Vec<_>>(),
                    "body": body,
                    "timings": resp.timings,
                    "sizes": resp.sizes,
                    "trace": hide_json(serde_json::to_value(&resp.trace).unwrap_or(Value::Null), &secrets),
                });
                (
                    Some(resp.status),
                    Some((resp.timings.total.as_secs_f64() * 1_000_000.0).round() / 1000.0),
                    None,
                    Some(response),
                )
            }
            Err(e) => {
                let response = e.trace().map(|t| {
                    json!({ "trace": hide_json(serde_json::to_value(t).unwrap_or(Value::Null), &secrets) })
                });
                (None, None, Some(hide(&e.to_string(), &secrets)), response)
            }
        };
        Self {
            method: req.method.to_string(),
            url,
            status,
            duration_ms,
            error,
            collection: source.map(|(c, _)| c.to_owned()),
            request_key: source.map(|(_, k)| k.to_owned()),
            request,
            response,
        }
    }
}

impl RunEntry {
    /// The run's detailed JSON report (already redacted by `Report`).
    pub fn from_report(report: &Report<'_>) -> Self {
        Self {
            kind: report.kind.to_owned(),
            target_key: report.key.clone(),
            name: report.name.clone(),
            passed: report.passed(),
            report: report.json(true),
            junit: Some(report.junit()),
            html: Some(report.html()),
        }
    }
}

/// The database.
#[derive(Debug, Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    /// Opens (or creates) the database and applies pending migrations.
    pub async fn open(path: &Path) -> Result<Self> {
        let path = path.to_path_buf();
        let conn = blocking(move || {
            let conn = Connection::open(&path)
                .map_err(|e| Error::Storage(format!("cannot open `{}`: {e}", path.display())))?;
            conn.execute_batch(
                "PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;",
            )
            .map_err(db_err)?;
            migrate(&conn)?;
            Ok(conn)
        })
        .await?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// An in-memory database (tests).
    pub async fn in_memory() -> Result<Self> {
        let conn = blocking(|| {
            let conn = Connection::open_in_memory().map_err(db_err)?;
            migrate(&conn)?;
            Ok(conn)
        })
        .await?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    async fn with<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let conn = self.conn.clone();
        blocking(move || {
            let mut guard = conn
                .lock()
                .map_err(|_| Error::Storage("the database lock is poisoned".into()))?;
            f(&mut guard)
        })
        .await
    }

    /// The schema version (number of applied migrations).
    pub async fn schema_version(&self) -> Result<u32> {
        self.with(|c| {
            c.query_row("PRAGMA user_version", [], |r| r.get(0))
                .map_err(db_err)
        })
        .await
    }

    pub async fn add_history(&self, e: HistoryEntry) -> Result<i64> {
        self.with(move |c| {
            let request = e.request.to_string();
            let response = e.response.as_ref().map(Value::to_string);
            let size = (request.len() + response.as_ref().map_or(0, String::len) + e.url.len()) as i64;
            c.execute(
                "INSERT INTO history (created_at, method, url, status, duration_ms, error, collection, request_key, request_json, response_json, size_bytes)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![now_ms(), e.method, e.url, e.status, e.duration_ms, e.error, e.collection, e.request_key, request, response, size],
            )
            .map_err(db_err)?;
            Ok(c.last_insert_rowid())
        })
        .await
    }

    /// History, newest first.
    pub async fn history(&self, q: HistoryQuery) -> Result<Vec<HistorySummary>> {
        self.with(move |c| {
            let mut sql = String::from(
                "SELECT id, created_at, method, url, status, duration_ms, error, collection, request_key FROM history WHERE 1 = 1",
            );
            let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            if let Some(u) = q.url_contains {
                sql.push_str(" AND instr(lower(url), lower(?)) > 0");
                args.push(Box::new(u));
            }
            if let Some(m) = q.method {
                sql.push_str(" AND method = ?");
                args.push(Box::new(m.to_ascii_uppercase()));
            }
            if let Some(s) = q.status {
                sql.push_str(" AND status = ?");
                args.push(Box::new(s));
            }
            if let Some(class) = q.status_class {
                sql.push_str(" AND status BETWEEN ? AND ?");
                args.push(Box::new(class * 100));
                args.push(Box::new(class * 100 + 99));
            }
            match q.failed {
                Some(true) => sql.push_str(" AND status IS NULL"),
                Some(false) => sql.push_str(" AND status IS NOT NULL"),
                None => {}
            }
            if let Some(coll) = q.collection {
                sql.push_str(" AND collection = ?");
                args.push(Box::new(coll));
            }
            if let Some(b) = q.before_id {
                sql.push_str(" AND id < ?");
                args.push(Box::new(b));
            }
            sql.push_str(" ORDER BY id DESC LIMIT ?");
            args.push(Box::new(q.limit.clamp(1, 1000) as i64));
            let mut stmt = c.prepare(&sql).map_err(db_err)?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(args.iter()), summary_row)
                .map_err(db_err)?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
        })
        .await
    }

    /// Deletes one entry (`id`) or every entry (`None`); returns how many were deleted.
    pub async fn delete_history(&self, id: Option<i64>) -> Result<usize> {
        self.with(move |c| {
            match id {
                Some(id) => c.execute("DELETE FROM history WHERE id = ?1", [id]),
                None => c.execute("DELETE FROM history", []),
            }
            .map_err(db_err)
        })
        .await
    }

    pub async fn history_entry(&self, id: i64) -> Result<Option<HistoryRecord>> {
        self.with(move |c| {
            c.query_row(
                "SELECT id, created_at, method, url, status, duration_ms, error, collection, request_key, request_json, response_json FROM history WHERE id = ?1",
                [id],
                |r| {
                    let request: String = r.get(9)?;
                    let response: Option<String> = r.get(10)?;
                    Ok(HistoryRecord {
                        summary: summary_row(r)?,
                        request: serde_json::from_str(&request).unwrap_or(Value::Null),
                        response: response.and_then(|t| serde_json::from_str(&t).ok()),
                    })
                },
            )
            .optional()
            .map_err(db_err)
        })
        .await
    }

    pub async fn add_run(&self, e: RunEntry) -> Result<i64> {
        self.with(move |c| {
            let report = e.report.to_string();
            let size = (report.len()
                + e.junit.as_ref().map_or(0, String::len)
                + e.html.as_ref().map_or(0, String::len)) as i64;
            c.execute(
                "INSERT INTO runs (created_at, kind, target_key, name, passed, report_json, size_bytes, junit, html) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![now_ms(), e.kind, e.target_key, e.name, e.passed, report, size, e.junit, e.html],
            )
            .map_err(db_err)?;
            Ok(c.last_insert_rowid())
        })
        .await
    }

    /// Runs, newest first.
    pub async fn runs(&self, limit: usize) -> Result<Vec<RunSummary>> {
        self.with(move |c| {
            let mut stmt = c
                .prepare("SELECT id, created_at, kind, target_key, name, passed FROM runs ORDER BY id DESC LIMIT ?1")
                .map_err(db_err)?;
            let rows = stmt
                .query_map([limit.clamp(1, 1000) as i64], |r| {
                    Ok(RunSummary {
                        id: r.get(0)?,
                        created_at: r.get(1)?,
                        kind: r.get(2)?,
                        target_key: r.get(3)?,
                        name: r.get(4)?,
                        passed: r.get(5)?,
                    })
                })
                .map_err(db_err)?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
        })
        .await
    }

    /// The stored report of a run.
    pub async fn run_report(&self, id: i64) -> Result<Option<Value>> {
        self.with(move |c| {
            let text: Option<String> = c
                .query_row("SELECT report_json FROM runs WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .optional()
                .map_err(db_err)?;
            Ok(text.and_then(|t| serde_json::from_str(&t).ok()))
        })
        .await
    }

    /// A stored rendering of a run's report (`None`: no such run, or not stored for it).
    pub async fn run_rendering(&self, id: i64, format: ReportFormat) -> Result<Option<String>> {
        let column = match format {
            ReportFormat::Json => "report_json",
            ReportFormat::Junit => "junit",
            ReportFormat::Html => "html",
        };
        self.with(move |c| {
            c.query_row(
                &format!("SELECT {column} FROM runs WHERE id = ?1"),
                [id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()
            .map(Option::flatten)
            .map_err(db_err)
        })
        .await
    }

    /// Deletes what `retention` does not keep: entries older than `max_age`, then the
    /// oldest entries (history and runs together) until the total size fits.
    pub async fn prune(&self, retention: Retention) -> Result<Pruned> {
        self.with(move |c| {
            let tx = c.transaction().map_err(db_err)?;
            let mut pruned = Pruned::default();
            if let Some(age) = retention.max_age {
                let cutoff = now_ms() - i64::try_from(age.as_millis()).unwrap_or(i64::MAX);
                pruned.history += tx.execute("DELETE FROM history WHERE created_at < ?1", [cutoff]).map_err(db_err)?;
                pruned.runs += tx.execute("DELETE FROM runs WHERE created_at < ?1", [cutoff]).map_err(db_err)?;
            }
            if let Some(max) = retention.max_bytes {
                let max = i64::try_from(max).unwrap_or(i64::MAX);
                let total = |tx: &rusqlite::Transaction<'_>| -> Result<i64> {
                    tx.query_row(
                        "SELECT (SELECT coalesce(sum(size_bytes), 0) FROM history) + (SELECT coalesce(sum(size_bytes), 0) FROM runs)",
                        [],
                        |r| r.get(0),
                    )
                    .map_err(db_err)
                };
                let mut over = total(&tx)? - max;
                while over > 0 {
                    // The oldest entry of either table.
                    let oldest: Option<(String, i64, i64)> = tx
                        .query_row(
                            "SELECT t, id, size_bytes FROM (
                                SELECT 'history' AS t, id, created_at, size_bytes FROM history
                                UNION ALL SELECT 'runs', id, created_at, size_bytes FROM runs
                             ) ORDER BY created_at, id LIMIT 1",
                            [],
                            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                        )
                        .optional()
                        .map_err(db_err)?;
                    let Some((table, id, size)) = oldest else { break };
                    if table == "history" {
                        tx.execute("DELETE FROM history WHERE id = ?1", [id]).map_err(db_err)?;
                        pruned.history += 1;
                    } else {
                        tx.execute("DELETE FROM runs WHERE id = ?1", [id]).map_err(db_err)?;
                        pruned.runs += 1;
                    }
                    over -= size;
                }
            }
            tx.commit().map_err(db_err)?;
            Ok(pruned)
        })
        .await
    }

    /// Sets an entry's timestamp (tests of retention).
    #[doc(hidden)]
    pub async fn backdate(&self, table: &'static str, id: i64, created_at: i64) -> Result<()> {
        self.with(move |c| {
            let sql = match table {
                "history" => "UPDATE history SET created_at = ?1 WHERE id = ?2",
                _ => "UPDATE runs SET created_at = ?1 WHERE id = ?2",
            };
            c.execute(sql, params![created_at, id]).map_err(db_err)?;
            Ok(())
        })
        .await
    }
}

fn summary_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<HistorySummary> {
    Ok(HistorySummary {
        id: r.get(0)?,
        created_at: r.get(1)?,
        method: r.get(2)?,
        url: r.get(3)?,
        status: r.get(4)?,
        duration_ms: r.get(5)?,
        error: r.get(6)?,
        collection: r.get(7)?,
        request_key: r.get(8)?,
    })
}

fn migrate(conn: &Connection) -> Result<()> {
    let current: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(db_err)?;
    let known = MIGRATIONS.len() as u32;
    if current > known {
        return Err(Error::Storage(format!(
            "the database was created by a newer SnapRequest (schema {current}, this version knows {known})"
        )));
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let version = i as u32 + 1;
        conn.execute_batch(&format!(
            "BEGIN; {sql}; PRAGMA user_version = {version}; COMMIT;"
        ))
        .map_err(|e| Error::Storage(format!("migration {version} failed: {e}")))?;
    }
    Ok(())
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| Error::Storage(format!("database task failed: {e}")))?
}
