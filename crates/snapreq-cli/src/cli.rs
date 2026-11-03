//! Command-line definitions.

use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};

/// Fast, self-hosted HTTP API testing and debugging.
#[derive(Debug, Parser)]
#[command(name = "snapreq", version, about, long_about = None)]
pub(crate) struct Cli {
    /// Internal log level for SnapRequest itself (error, warn, info, debug, trace).
    /// Overridden by the `SNAPREQ_LOG` environment variable.
    #[arg(long, global = true, default_value = "warn")]
    pub log_level: String,

    #[command(subcommand)]
    pub command: Command,
}

// Parsed once per process; boxing `SendArgs` would only add noise.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Send one HTTP request and show the response.
    ///
    /// Examples:
    ///   snapreq send https://httpbin.org/get
    ///   snapreq send POST https://api.example.com/users --json '{"name":"snap"}'
    ///   snapreq send PUT https://api.example.com/avatar -F image=@me.png -H 'Authorization: Bearer TOKEN'
    ///   snapreq send my-api/users/create-user.toml -e dev --var name=snap
    ///   snapreq send -c my-api users/create-user -e prod
    #[command(verbatim_doc_comment)]
    Send(SendArgs),

    /// Run a flow, a folder or a whole collection, with checks and captures.
    ///
    /// Examples:
    ///   snapreq run my-api/orders/checkout.flow.toml -e staging
    ///   snapreq run -c my-api orders/checkout --var item=sku-1
    ///   snapreq run my-api                     # every request of the collection
    ///   snapreq run -c my-api --folder users --data users.csv
    #[command(verbatim_doc_comment)]
    Run(RunArgs),

    /// List the folders, requests and flows of a collection.
    Ls(LsArgs),

    /// Create a collection or a request file.
    #[command(subcommand)]
    New(NewCommand),

    /// Manage secret values (stored outside the collection, never committed).
    #[command(subcommand)]
    Secret(SecretCommand),

    /// Manage cached OAuth 2.0 tokens.
    #[command(subcommand)]
    Tokens(TokensCommand),
}

#[derive(Debug, Subcommand)]
pub(crate) enum TokensCommand {
    /// Delete every cached OAuth 2.0 token (sign out everywhere).
    Clear,
    /// Print the path of the token cache file.
    Path,
}

#[derive(Debug, Args)]
pub(crate) struct RunArgs {
    /// A flow file, a collection or folder directory, or (with `-c`, or from inside a
    /// collection) a flow or folder key. Default: the collection around the current directory.
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,

    /// Collection directory; TARGET is then a flow or folder key.
    #[arg(short = 'c', long = "collection", value_name = "DIR")]
    pub collection: Option<PathBuf>,

    /// Run only the requests of this folder (`users`, `users/admin`).
    #[arg(long, value_name = "FOLDER")]
    pub folder: Option<String>,

    /// Run everything this many times (fresh captures each time).
    #[arg(short = 'n', long, value_name = "N", conflicts_with = "data")]
    pub iterations: Option<usize>,

    /// Data-driven run: one iteration per row of a CSV (header row = variable names) or
    /// JSON (array of objects) file.
    #[arg(long, value_name = "FILE")]
    pub data: Option<PathBuf>,

    /// Environment to use (`environments/<NAME>.toml`).
    #[arg(short = 'e', long = "env", value_name = "NAME")]
    pub env: Option<String>,

    /// Set a variable for the whole run, `name=value` (wins over flow and step variables).
    #[arg(long = "var", value_name = "NAME=VALUE")]
    pub vars: Vec<String>,

    /// Stop at the first failing request (flows always stop at a failing step); later
    /// iterations are skipped.
    #[arg(long)]
    pub bail: bool,

    /// Pause between requests (and flow steps), in milliseconds.
    #[arg(long, value_name = "MS")]
    pub delay: Option<u64>,

    /// Run up to N iterations (data rows) at the same time. Requests within an iteration
    /// always run in order.
    #[arg(long, value_name = "N", default_value = "1")]
    pub parallel: usize,

    /// Show every check and capture; `-vv` also the trace of a failing step with wire bytes.
    #[arg(short = 'v', long = "verbose", action = ArgAction::Count)]
    pub verbose: u8,

    /// Output format.
    #[arg(long, value_name = "FORMAT", default_value = "pretty")]
    pub format: FormatArg,

    #[arg(long, value_name = "WHEN", default_value = "auto")]
    pub color: ColorArg,

    /// Skip TLS certificate verification for every request.
    #[arg(short = 'k', long)]
    pub insecure: bool,

    /// Total time limit per request in seconds.
    #[arg(short = 't', long, value_name = "SECONDS")]
    pub timeout: Option<f64>,

    /// Value to hide in all output. Repeatable.
    #[arg(long = "secret", value_name = "VALUE")]
    pub secrets: Vec<String>,

    /// Write a report: `junit:FILE`, `html:FILE` or `json:FILE` (with full traces). Repeatable.
    #[arg(long = "report", value_name = "FORMAT:FILE")]
    pub reports: Vec<String>,
}

/// Report formats of `snapreq run --report`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReportFormat {
    Junit,
    Html,
    Json,
}

#[derive(Debug, Args)]
pub(crate) struct LsArgs {
    /// Collection directory (default: the current one, or the nearest parent with `snapreq.toml`).
    #[arg(value_name = "DIR")]
    pub dir: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum NewCommand {
    /// Create a new collection in DIR.
    Collection {
        /// Directory to create (must not contain a collection yet).
        dir: PathBuf,
        /// Collection name (default: the directory name).
        #[arg(long)]
        name: Option<String>,
    },
    /// Create a request file inside a collection.
    Request {
        /// Path of the new file, e.g. `my-api/users/get-user.toml`.
        path: PathBuf,
        /// `[METHOD] URL`.
        #[arg(value_name = "METHOD_OR_URL", num_args = 1..=2, required = true)]
        target: Vec<String>,
        /// Display name.
        #[arg(long)]
        name: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum SecretCommand {
    /// Store a secret value in `.secrets/<env>.toml` (or `.secrets/shared.toml`).
    /// The value is read from a hidden prompt, or from stdin when it is not a terminal.
    Set {
        /// Variable name.
        name: String,
        /// Environment the value is for (default: shared by all environments).
        #[arg(short = 'e', long = "env")]
        env: Option<String>,
        /// Collection directory (default: the current one, or the nearest parent).
        #[arg(short = 'c', long = "collection", value_name = "DIR")]
        collection: Option<PathBuf>,
    },
}

#[derive(Debug, Args)]
pub(crate) struct SendArgs {
    /// `[METHOD] URL`, a request file (`dir/req.toml`), or a request key with `-c`.
    /// The method defaults to GET, or POST when a body is given.
    #[arg(value_name = "TARGET", num_args = 1..=2, required = true)]
    pub target: Vec<String>,

    // --- Collection --------------------------------------------------------------
    /// Collection directory; TARGET is then a request key such as `users/create-user`.
    #[arg(
        short = 'c',
        long = "collection",
        value_name = "DIR",
        help_heading = "Collection"
    )]
    pub collection: Option<PathBuf>,

    /// Environment to use (`environments/<NAME>.toml`).
    #[arg(
        short = 'e',
        long = "env",
        value_name = "NAME",
        help_heading = "Collection"
    )]
    pub env: Option<String>,

    /// Do not run the request file's `[assert]` checks.
    #[arg(long = "no-assert", help_heading = "Collection")]
    pub no_assert: bool,

    /// Set a variable for this run, `name=value` (highest precedence). Repeatable.
    #[arg(long = "var", value_name = "NAME=VALUE", help_heading = "Collection")]
    pub vars: Vec<String>,

    // --- Auth --------------------------------------------------------------------
    /// Basic auth, `USER[:PASSWORD]`. Without a password you are asked for one
    /// (on a terminal), which keeps it out of the shell history.
    #[arg(
        short = 'u',
        long = "user",
        value_name = "USER[:PASSWORD]",
        help_heading = "Auth",
        conflicts_with_all = ["bearer", "api_key", "no_auth"]
    )]
    pub user: Option<String>,

    /// Use Digest auth with the `-u` credentials instead of Basic.
    #[arg(long, requires = "user", help_heading = "Auth")]
    pub digest: bool,

    /// Bearer token auth (`Authorization: Bearer TOKEN`).
    #[arg(
        long,
        value_name = "TOKEN",
        help_heading = "Auth",
        conflicts_with_all = ["api_key", "no_auth"]
    )]
    pub bearer: Option<String>,

    /// API key auth, `NAME=VALUE`; sent as a header unless `--api-key-in query`.
    #[arg(
        long = "api-key",
        value_name = "NAME=VALUE",
        help_heading = "Auth",
        conflicts_with = "no_auth"
    )]
    pub api_key: Option<String>,

    /// Where `--api-key` goes.
    #[arg(
        long = "api-key-in",
        value_name = "WHERE",
        default_value = "header",
        requires = "api_key",
        help_heading = "Auth"
    )]
    pub api_key_in: ApiKeyInArg,

    /// OAuth 2.0 with this grant; needs `--token-url` and `--client-id`. The password grant
    /// takes the user's credentials from `-u`. Tokens are cached and refreshed (see
    /// `snapreq tokens`).
    #[arg(
        long = "oauth2",
        value_name = "GRANT",
        help_heading = "Auth",
        conflicts_with_all = ["bearer", "api_key", "no_auth", "digest"]
    )]
    pub oauth2: Option<OAuth2GrantArg>,

    /// OAuth 2.0 token endpoint.
    #[arg(
        long = "token-url",
        value_name = "URL",
        requires = "oauth2",
        help_heading = "Auth"
    )]
    pub token_url: Option<String>,

    /// OAuth 2.0 authorization endpoint (authorization code grant).
    #[arg(
        long = "auth-url",
        value_name = "URL",
        requires = "oauth2",
        help_heading = "Auth"
    )]
    pub auth_url: Option<String>,

    /// OAuth 2.0 client id.
    #[arg(
        long = "client-id",
        value_name = "ID",
        requires = "oauth2",
        help_heading = "Auth"
    )]
    pub client_id: Option<String>,

    /// OAuth 2.0 client secret (omit for public clients).
    #[arg(
        long = "client-secret",
        value_name = "SECRET",
        requires = "oauth2",
        help_heading = "Auth"
    )]
    pub client_secret: Option<String>,

    /// OAuth 2.0 scopes, space-separated.
    #[arg(
        long = "scope",
        value_name = "SCOPES",
        requires = "oauth2",
        help_heading = "Auth"
    )]
    pub scope: Option<String>,

    /// Loopback redirect URI for the authorization code grant
    /// (default `http://127.0.0.1:<random port>/callback`).
    #[arg(
        long = "redirect-uri",
        value_name = "URI",
        requires = "oauth2",
        help_heading = "Auth"
    )]
    pub redirect_uri: Option<String>,

    /// AWS Signature V4 for `REGION/SERVICE` (for example `eu-west-1/s3`). Credentials come
    /// from `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` and optionally `AWS_SESSION_TOKEN`.
    #[arg(
        long = "aws-sigv4",
        value_name = "REGION/SERVICE",
        help_heading = "Auth",
        conflicts_with_all = ["user", "bearer", "api_key", "no_auth", "oauth2"]
    )]
    pub aws_sigv4: Option<String>,

    /// Send no auth, even if the collection or folder defines one.
    #[arg(long = "no-auth", help_heading = "Auth")]
    pub no_auth: bool,

    // --- Request -----------------------------------------------------------------
    /// Request header, `Name: value`. Repeatable.
    #[arg(
        short = 'H',
        long = "header",
        value_name = "HEADER",
        help_heading = "Request"
    )]
    pub headers: Vec<String>,

    /// Query parameter, `name=value`. Repeatable.
    #[arg(
        short = 'q',
        long = "query",
        value_name = "NAME=VALUE",
        help_heading = "Request"
    )]
    pub query: Vec<String>,

    /// Raw body text, or `@path` to send a file.
    #[arg(
        short = 'd',
        long = "data",
        value_name = "BODY",
        group = "body",
        help_heading = "Request"
    )]
    pub data: Option<String>,

    /// JSON body text, or `@path` to send a JSON file. Sets `Content-Type: application/json`.
    #[arg(long, value_name = "JSON", group = "body", help_heading = "Request")]
    pub json: Option<String>,

    /// URL-encoded form field, `name=value`. Repeatable.
    #[arg(
        long = "form",
        value_name = "NAME=VALUE",
        group = "body",
        help_heading = "Request"
    )]
    pub form: Vec<String>,

    /// Multipart field, `name=value` or `name=@path[;type=mime]`. Repeatable.
    #[arg(
        short = 'F',
        long = "multipart",
        value_name = "FIELD",
        group = "body",
        help_heading = "Request"
    )]
    pub multipart: Vec<String>,

    /// Send this file as the raw body.
    #[arg(long, value_name = "PATH", group = "body", help_heading = "Request")]
    pub file: Option<PathBuf>,

    // --- Connection ---------------------------------------------------------------
    /// Total time limit in seconds (0 = none). Default 30.
    #[arg(
        short = 't',
        long,
        value_name = "SECONDS",
        allow_negative_numbers = true,
        help_heading = "Connection"
    )]
    pub timeout: Option<f64>,

    /// Connection time limit in seconds (DNS + TCP + TLS; 0 = none). Default 10.
    #[arg(
        long,
        value_name = "SECONDS",
        allow_negative_numbers = true,
        help_heading = "Connection"
    )]
    pub connect_timeout: Option<f64>,

    /// Use HTTP/1.1 only.
    #[arg(
        long = "http1.1",
        conflicts_with = "http2",
        help_heading = "Connection"
    )]
    pub http1: bool,

    /// Require HTTP/2 (ALPN for HTTPS, prior knowledge for plain HTTP).
    #[arg(long = "http2", help_heading = "Connection")]
    pub http2: bool,

    /// Proxy URL (`http://`, `socks5://`, `socks5h://`), `direct`, or `env` (default).
    #[arg(short = 'x', long, value_name = "PROXY", help_heading = "Connection")]
    pub proxy: Option<String>,

    /// Pin a host to addresses: `host:port:addr[,addr]` (port may be `*`). Repeatable.
    #[arg(long, value_name = "HOST:PORT:ADDR", help_heading = "Connection")]
    pub resolve: Vec<String>,

    /// Do not follow redirects.
    #[arg(long, help_heading = "Connection")]
    pub no_follow: bool,

    /// Maximum number of redirects to follow. Default 10.
    #[arg(long, value_name = "N", help_heading = "Connection")]
    pub max_redirs: Option<u32>,

    /// Do not ask for or decode compressed responses.
    #[arg(long, help_heading = "Connection")]
    pub no_decompress: bool,

    /// Do not send or store cookies.
    #[arg(long, help_heading = "Connection")]
    pub no_cookies: bool,

    // --- TLS -----------------------------------------------------------------------
    /// Skip certificate verification (insecure). The trace still says what would have failed.
    #[arg(short = 'k', long, help_heading = "TLS")]
    pub insecure: bool,

    /// Extra CA certificates to trust (PEM file).
    #[arg(long, value_name = "PATH", help_heading = "TLS")]
    pub cacert: Option<PathBuf>,

    /// Client certificate for mutual TLS (PEM). Use with `--key`.
    #[arg(long, value_name = "PATH", requires = "key", help_heading = "TLS")]
    pub cert: Option<PathBuf>,

    /// Private key for `--cert` (PEM).
    #[arg(long, value_name = "PATH", requires = "cert", help_heading = "TLS")]
    pub key: Option<PathBuf>,

    /// Lowest TLS version allowed.
    #[arg(long, value_name = "VERSION", help_heading = "TLS")]
    pub tls_min: Option<TlsVersionArg>,

    /// Highest TLS version allowed.
    #[arg(long, value_name = "VERSION", help_heading = "TLS")]
    pub tls_max: Option<TlsVersionArg>,

    // --- Output --------------------------------------------------------------------
    /// More detail: `-v` request and timings, `-vv` full trace and raw wire bytes.
    #[arg(short = 'v', long = "verbose", action = ArgAction::Count, help_heading = "Output")]
    pub verbose: u8,

    /// What to print: `H` request headers, `B` request body, `h` response status and headers,
    /// `b` response body, `m` timings and sizes, `t` timing waterfall.
    /// Default `hb` on a terminal, `b` when piped.
    #[arg(
        short = 'p',
        long = "print",
        value_name = "PARTS",
        help_heading = "Output"
    )]
    pub print: Option<String>,

    /// Output format: `pretty` for people, `json` for one machine-readable document on stdout.
    #[arg(
        long,
        value_name = "FORMAT",
        default_value = "pretty",
        help_heading = "Output"
    )]
    pub format: FormatArg,

    /// Save the (decoded) response body to FILE instead of printing it. Overwrites FILE.
    #[arg(
        short = 'o',
        long = "output",
        value_name = "FILE",
        help_heading = "Output"
    )]
    pub output: Option<PathBuf>,

    /// When to use colors.
    #[arg(
        long,
        value_name = "WHEN",
        default_value = "auto",
        help_heading = "Output"
    )]
    pub color: ColorArg,

    /// Exit with code 4 when the response status is 400 or higher.
    #[arg(short = 'f', long, help_heading = "Output")]
    pub fail: bool,

    /// Value to hide in all output (for example a token). Repeatable.
    #[arg(long = "secret", value_name = "VALUE", help_heading = "Output")]
    pub secrets: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum OAuth2GrantArg {
    ClientCredentials,
    Password,
    AuthorizationCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ApiKeyInArg {
    Header,
    Query,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum TlsVersionArg {
    #[value(name = "1.2")]
    Tls12,
    #[value(name = "1.3")]
    Tls13,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ColorArg {
    /// Colors on a terminal, unless `NO_COLOR` is set.
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum FormatArg {
    Pretty,
    Json,
}
