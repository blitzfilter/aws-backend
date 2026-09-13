use percent_encoding::percent_decode_str;
use rustls::{
    RootCertStore,
    pki_types::{CertificateDer, pem::PemObject},
};
use sqlx::{
    ConnectOptions, PgConnection, PgPool,
    postgres::{PgConnectOptions, PgPoolOptions, PgSslMode},
};
use std::{
    fmt,
    fs::OpenOptions,
    io::{self, Read},
    net::IpAddr,
    time::Duration,
};
use url::{Host, Url};

const DEFAULT_PORT: u16 = 5432;
const DEFAULT_MAX_CONNECTIONS: u32 = 2;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ROOT_CERT_BYTES: usize = 1024 * 1024;
const MAX_PASSWORD_BYTES: usize = 16 * 1024;
const MAX_URL_BYTES: usize = 64 * 1024;
const UNSUPPORTED_AMBIENT: [&str; 4] = ["PGSSLCERT", "PGSSLKEY", "PGSSLROOTCERT", "PGOPTIONS"];

/// Protected TLS policy parsed from explicit inputs supplied by the composition root.
#[derive(Clone)]
pub struct PostgresTlsConfig {
    mode: PgSslMode,
    root_pem: Option<Vec<u8>>,
    application_name: String,
}

impl fmt::Debug for PostgresTlsConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PostgresTlsConfig")
            .field("mode", &self.mode)
            .field("has_root_certificate", &self.root_pem.is_some())
            .finish_non_exhaustive()
    }
}

impl PostgresTlsConfig {
    pub fn new(
        stage: &str,
        mode: &str,
        root_pem: Option<Vec<u8>>,
        application_name: &str,
    ) -> Result<Self, PostgresPoolConfigError> {
        let local = match stage {
            "dev" | "prod" => false,
            "local" | "ephemeral" | "test" => true,
            _ => return Err(PostgresPoolConfigError::InvalidStage),
        };
        let mode = match mode {
            "verify-full" => PgSslMode::VerifyFull,
            "disable" if local => PgSslMode::Disable,
            "disable" => return Err(PostgresPoolConfigError::VerifyFullRequired),
            _ => return Err(PostgresPoolConfigError::InvalidSslMode),
        };
        if application_name.is_empty()
            || application_name.len() >= 64
            || !application_name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || !application_name.as_bytes()[0].is_ascii_alphanumeric()
        {
            return Err(PostgresPoolConfigError::InvalidApplicationName);
        }
        match (mode, root_pem.as_deref()) {
            (PgSslMode::VerifyFull, Some(pem)) => validate_root_pem(pem)?,
            (PgSslMode::VerifyFull, None) => {
                return Err(PostgresPoolConfigError::RootCertificateRequired);
            }
            (PgSslMode::Disable, Some(_)) => {
                return Err(PostgresPoolConfigError::UnexpectedRootCertificate);
            }
            _ => {}
        }
        Ok(Self {
            mode,
            root_pem,
            application_name: application_name.to_owned(),
        })
    }

    /// `get` must expose the actual process environment in production, including PG* keys.
    /// Root certificates use POSTGRES_SSL_ROOT_CERT (file path), not URL query parameters.
    pub fn from_lookup(
        app: &str,
        mut get: impl FnMut(&'static str) -> Option<String>,
    ) -> Result<Self, PostgresPoolConfigError> {
        for key in UNSUPPORTED_AMBIENT {
            if get(key).is_some() {
                return Err(PostgresPoolConfigError::UnsupportedAmbientSetting(key));
            }
        }
        let stage = required(&mut get, "STAGE")?;
        let mode = required(&mut get, "POSTGRES_SSL_MODE")?;
        let root_pem = get("POSTGRES_SSL_ROOT_CERT")
            .map(|path| read_file(&path, "POSTGRES_SSL_ROOT_CERT", MAX_ROOT_CERT_BYTES, false))
            .transpose()?;
        Self::new(&stage, &mode, root_pem, app)
    }
}

/// Validated, snapshotted SQLx options. Construction transiently reads SQLx's ambient
/// defaults, then overwrites supported fields and rejects unsupported settings.
/// Never log the returned raw SQLx options or pool.
#[derive(Clone)]
pub struct PostgresPoolConfig {
    options: PgConnectOptions,
    max_connections: u32,
}

impl fmt::Debug for PostgresPoolConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PostgresPoolConfig")
            .field("ssl_mode", &self.options.get_ssl_mode())
            .field("max_connections", &self.max_connections)
            .field("connection", &"<redacted>")
            .finish()
    }
}

impl PostgresPoolConfig {
    pub fn new(
        host: String,
        port: u16,
        database: String,
        username: String,
        password: String,
        max_connections: u32,
        tls: PostgresTlsConfig,
    ) -> Result<Self, PostgresPoolConfigError> {
        if max_connections == 0 {
            return Err(PostgresPoolConfigError::ZeroMaxConnections);
        }
        validate_host(&host)?;
        if port == 0 {
            return Err(PostgresPoolConfigError::InvalidInput("POSTGRES_PORT"));
        }
        validate_text(&database, "POSTGRES_DATABASE", 63)?;
        validate_text(&username, "POSTGRES_USERNAME", 63)?;
        validate_text(&password, "POSTGRES_PASSWORD", MAX_PASSWORD_BYTES)?;
        let options = ambient_checked_options()?
            .host(&host)
            .port(port)
            .database(&database)
            .username(&username)
            .password(&password)
            .ssl_mode(tls.mode)
            .ssl_root_cert_from_pem(tls.root_pem.unwrap_or_default())
            .application_name(&tls.application_name)
            .disable_statement_logging();
        Ok(Self {
            options,
            max_connections,
        })
    }

    pub fn from_lookup(
        app: &str,
        mut get: impl FnMut(&'static str) -> Option<String>,
    ) -> Result<Self, PostgresPoolConfigError> {
        let tls = PostgresTlsConfig::from_lookup(app, &mut get)?;
        let host = required(&mut get, "POSTGRES_HOST")?;
        let database = required(&mut get, "POSTGRES_DATABASE")?;
        let username = required(&mut get, "POSTGRES_USERNAME")?;
        let password = match (get("POSTGRES_PASSWORD"), get("POSTGRES_PASSWORD_FILE")) {
            (Some(_), Some(_)) => return Err(PostgresPoolConfigError::ConflictingPasswordInputs),
            (Some(password), None) => password,
            (None, Some(path)) => {
                let bytes = read_file(&path, "POSTGRES_PASSWORD_FILE", MAX_PASSWORD_BYTES, true)?;
                let password = String::from_utf8(bytes)
                    .map_err(|_| PostgresPoolConfigError::InvalidInput("POSTGRES_PASSWORD_FILE"))?;
                // Permit a secret provisioned by echo. Never trim meaningful password spaces.
                password
                    .strip_suffix("\r\n")
                    .or_else(|| password.strip_suffix('\n'))
                    .unwrap_or(&password)
                    .to_owned()
            }
            (None, None) => {
                return Err(PostgresPoolConfigError::MissingInput(
                    "POSTGRES_PASSWORD or POSTGRES_PASSWORD_FILE",
                ));
            }
        };
        let port = optional_number(&mut get, "POSTGRES_PORT", DEFAULT_PORT)?;
        let max = optional_number(
            &mut get,
            "POSTGRES_MAX_CONNECTIONS",
            DEFAULT_MAX_CONNECTIONS,
        )?;
        Self::new(host, port, database, username, password, max, tls)
    }

    /// Only sslmode and application_name query options are accepted, once each, and must
    /// match protected policy. Credentials and database must be explicit in the URL.
    pub fn from_url(
        url: &str,
        max: u32,
        tls: PostgresTlsConfig,
    ) -> Result<Self, PostgresPoolConfigError> {
        if url.len() > MAX_URL_BYTES
            || !url.is_ascii()
            || url
                .bytes()
                .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
        {
            return Err(PostgresPoolConfigError::InvalidUrl);
        }
        validate_percent_encoding(url)?;
        let (scheme, rest) = url
            .split_once("://")
            .ok_or(PostgresPoolConfigError::InvalidUrl)?;
        if !matches!(scheme, "postgres" | "postgresql") {
            return Err(PostgresPoolConfigError::InvalidUrl);
        }
        let (authority, path_query) = rest
            .split_once('/')
            .ok_or(PostgresPoolConfigError::InvalidUrl)?;
        if authority.matches('@').count() != 1 || authority.ends_with(':') {
            return Err(PostgresPoolConfigError::InvalidUrl);
        }
        let path = path_query
            .split('?')
            .next()
            .ok_or(PostgresPoolConfigError::InvalidUrl)?;
        if path.is_empty() || path.contains('/') || matches!(decode(path)?.as_str(), "." | "..") {
            return Err(PostgresPoolConfigError::InvalidUrl);
        }
        let parsed = Url::parse(url).map_err(|_| PostgresPoolConfigError::InvalidUrl)?;
        if parsed.fragment().is_some() {
            return Err(PostgresPoolConfigError::InvalidUrl);
        }
        let host = match parsed.host().ok_or(PostgresPoolConfigError::InvalidUrl)? {
            Host::Domain(host) => host.to_owned(),
            Host::Ipv4(host) => host.to_string(),
            Host::Ipv6(host) => host.to_string(),
        };
        let password = parsed
            .password()
            .ok_or(PostgresPoolConfigError::InvalidUrl)?;
        let mut seen_mode = false;
        let mut seen_app = false;
        if let Some(query) = parsed.query() {
            if query.is_empty() || query.split('&').any(|pair| !pair.contains('=')) {
                return Err(PostgresPoolConfigError::InvalidUrl);
            }
            for (key, value) in parsed.query_pairs() {
                let (seen, matches_policy) = match key.as_ref() {
                    "sslmode" => (
                        &mut seen_mode,
                        value
                            == match tls.mode {
                                PgSslMode::VerifyFull => "verify-full",
                                _ => "disable",
                            },
                    ),
                    "application_name" => (&mut seen_app, value == tls.application_name),
                    _ => return Err(PostgresPoolConfigError::UnsupportedUrlOption),
                };
                if *seen {
                    return Err(PostgresPoolConfigError::DuplicateUrlOption);
                }
                *seen = true;
                if !matches_policy {
                    return Err(PostgresPoolConfigError::UrlPolicyMismatch);
                }
            }
        }
        Self::new(
            host,
            parsed.port().unwrap_or(DEFAULT_PORT),
            decode(path)?,
            decode(parsed.username())?,
            decode(password)?,
            max,
            tls,
        )
    }

    pub fn host(&self) -> &str {
        self.options.get_host()
    }
    pub fn port(&self) -> u16 {
        self.options.get_port()
    }
    pub fn database(&self) -> &str {
        self.options.get_database().unwrap_or_default()
    }
    pub fn username(&self) -> &str {
        self.options.get_username()
    }
    pub const fn max_connections(&self) -> u32 {
        self.max_connections
    }

    /// Contains secrets and SQLx's non-redacted Debug implementation. Do not log it.
    pub fn connect_options(&self) -> PgConnectOptions {
        self.options.clone()
    }

    pub fn pool_options(&self) -> PgPoolOptions {
        PgPoolOptions::new()
            .max_connections(self.max_connections)
            .acquire_timeout(CONNECTION_TIMEOUT)
    }

    /// Opens a dedicated session for session-scoped locks or migrations, not a pooled
    /// transaction connection. Establishment has the shared five-second deadline;
    /// the caller owns the session lifetime and any subsequent query deadlines.
    pub async fn connect_session(&self) -> Result<PgConnection, PostgresConnectError> {
        tokio::time::timeout(CONNECTION_TIMEOUT, self.options.connect())
            .await
            .map_err(|elapsed| {
                PostgresConnectError::from(sqlx::Error::Io(io::Error::new(
                    io::ErrorKind::TimedOut,
                    elapsed,
                )))
            })?
            .map_err(PostgresConnectError::from)
    }

    pub async fn connect(&self) -> Result<PgPool, PostgresConnectError> {
        self.pool_options()
            .connect_with(self.connect_options())
            .await
            .map_err(PostgresConnectError::from)
    }
}

fn ambient_checked_options() -> Result<PgConnectOptions, PostgresPoolConfigError> {
    // SQLx 0.9 has no env-free constructor or client-cert getters/resetters. Inspect its
    // documented URL projection, using fixed authority fields so hostile PGUSER/PGHOST
    // cannot panic SQLx's URL builder. Never parse or log this temporary URL. No pgpass.
    let options = PgConnectOptions::new_without_pgpass()
        .host("localhost")
        .port(DEFAULT_PORT)
        .username("policy")
        .password("")
        .database("policy");
    if options.get_options().is_some() {
        return Err(PostgresPoolConfigError::UnsupportedAmbientSetting(
            "PGOPTIONS",
        ));
    }
    for (key, _) in options.to_url_lossy().query_pairs() {
        let ambient = match key.as_ref() {
            "sslcert" => "PGSSLCERT",
            "sslkey" => "PGSSLKEY",
            "sslrootcert" => "PGSSLROOTCERT",
            _ => continue,
        };
        return Err(PostgresPoolConfigError::UnsupportedAmbientSetting(ambient));
    }
    Ok(options)
}

fn validate_root_pem(pem: &[u8]) -> Result<(), PostgresPoolConfigError> {
    if pem.len() > MAX_ROOT_CERT_BYTES {
        return Err(PostgresPoolConfigError::RootCertificateTooLarge);
    }
    let mut rest = std::str::from_utf8(pem)
        .map_err(|_| PostgresPoolConfigError::InvalidRootCertificate)?
        .trim_ascii();
    let mut roots = RootCertStore::empty();
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    while !rest.is_empty() {
        if !rest.starts_with(BEGIN) {
            return Err(PostgresPoolConfigError::InvalidRootCertificate);
        }
        let end = rest
            .find(END)
            .ok_or(PostgresPoolConfigError::InvalidRootCertificate)?
            + END.len();
        let cert = CertificateDer::from_pem_slice(&rest.as_bytes()[..end])
            .map_err(|_| PostgresPoolConfigError::InvalidRootCertificate)?;
        roots
            .add(cert)
            .map_err(|_| PostgresPoolConfigError::InvalidRootCertificate)?;
        rest = rest[end..].trim_ascii();
    }
    if roots.is_empty() {
        return Err(PostgresPoolConfigError::InvalidRootCertificate);
    }
    Ok(())
}

fn validate_host(host: &str) -> Result<(), PostgresPoolConfigError> {
    if host.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    let dns = host.strip_suffix('.').unwrap_or(host);
    if host.len() > 253
        || dns.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return Err(PostgresPoolConfigError::InvalidInput("POSTGRES_HOST"));
    }
    Ok(())
}

fn validate_text(
    text: &str,
    input: &'static str,
    max: usize,
) -> Result<(), PostgresPoolConfigError> {
    if text.trim().is_empty() || text.len() > max || text.chars().any(char::is_control) {
        return Err(PostgresPoolConfigError::InvalidInput(input));
    }
    Ok(())
}

fn required(
    get: &mut impl FnMut(&'static str) -> Option<String>,
    key: &'static str,
) -> Result<String, PostgresPoolConfigError> {
    get(key).ok_or(PostgresPoolConfigError::MissingInput(key))
}

fn optional_number<T: std::str::FromStr>(
    get: &mut impl FnMut(&'static str) -> Option<String>,
    key: &'static str,
    default: T,
) -> Result<T, PostgresPoolConfigError> {
    match get(key) {
        Some(value) if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) => value
            .parse()
            .map_err(|_| PostgresPoolConfigError::InvalidInput(key)),
        Some(_) => Err(PostgresPoolConfigError::InvalidInput(key)),
        None => Ok(default),
    }
}

fn validate_percent_encoding(value: &str) -> Result<(), PostgresPoolConfigError> {
    let bytes = value.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'%'
            && !bytes
                .get(i + 1..i + 3)
                .is_some_and(|pair| pair.iter().all(u8::is_ascii_hexdigit))
        {
            return Err(PostgresPoolConfigError::InvalidUrl);
        }
    }
    Ok(())
}

fn decode(value: &str) -> Result<String, PostgresPoolConfigError> {
    percent_decode_str(value)
        .decode_utf8()
        .map(|s| s.into_owned())
        .map_err(|_| PostgresPoolConfigError::InvalidUrl)
}

fn read_file(
    path: &str,
    input: &'static str,
    max: usize,
    secret: bool,
) -> Result<Vec<u8>, PostgresPoolConfigError> {
    if path.is_empty() {
        return Err(PostgresPoolConfigError::InvalidInput(input));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // No final-component symlinks; opening a FIFO must not block before the type check.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(not(unix))]
    if secret {
        return Err(PostgresPoolConfigError::SecretFilePermissions);
    }
    let file = options
        .open(path)
        .map_err(|source| PostgresPoolConfigError::FileRead { input, source })?;
    let metadata = file
        .metadata()
        .map_err(|source| PostgresPoolConfigError::FileRead { input, source })?;
    if !metadata.is_file() {
        return Err(PostgresPoolConfigError::NotRegularFile(input));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode() & 0o7777;
        if secret && !matches!(mode, 0o400 | 0o600) {
            return Err(PostgresPoolConfigError::SecretFilePermissions);
        }
        if mode & 0o444 == 0 {
            return Err(PostgresPoolConfigError::InvalidInput(input));
        }
    }
    if metadata.len() > max as u64 {
        return Err(PostgresPoolConfigError::FileTooLarge(input));
    }
    let mut bytes = Vec::new();
    file.take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| PostgresPoolConfigError::FileRead { input, source })?;
    if bytes.len() > max {
        return Err(PostgresPoolConfigError::FileTooLarge(input));
    }
    if bytes.is_empty() {
        return Err(PostgresPoolConfigError::InvalidInput(input));
    }
    Ok(bytes)
}

#[derive(thiserror::Error)]
pub enum PostgresPoolConfigError {
    #[error("missing required PostgreSQL input: {0}")]
    MissingInput(&'static str),
    #[error("invalid PostgreSQL input: {0}")]
    InvalidInput(&'static str),
    #[error("STAGE must be dev, prod, local, ephemeral, or test")]
    InvalidStage,
    #[error("POSTGRES_SSL_MODE must be verify-full or disable")]
    InvalidSslMode,
    #[error("dev and prod require PostgreSQL verify-full TLS")]
    VerifyFullRequired,
    #[error("verify-full requires an explicit PostgreSQL CA certificate")]
    RootCertificateRequired,
    #[error("disabled PostgreSQL TLS cannot take a CA certificate")]
    UnexpectedRootCertificate,
    #[error("invalid PostgreSQL CA certificate PEM bundle")]
    InvalidRootCertificate,
    #[error("PostgreSQL CA certificate exceeds 1 MiB")]
    RootCertificateTooLarge,
    #[error(
        "PostgreSQL application name must be 1-63 ASCII letters, digits, dots, underscores or hyphens, starting with a letter or digit"
    )]
    InvalidApplicationName,
    #[error("Postgres max connections must be greater than zero")]
    ZeroMaxConnections,
    #[error("POSTGRES_PASSWORD and POSTGRES_PASSWORD_FILE are mutually exclusive")]
    ConflictingPasswordInputs,
    #[error("unsupported ambient PostgreSQL setting: {0}")]
    UnsupportedAmbientSetting(&'static str),
    #[error(
        "invalid PostgreSQL URL; explicit scheme, TCP host, username, password and database required"
    )]
    InvalidUrl,
    #[error("unsupported PostgreSQL URL query option")]
    UnsupportedUrlOption,
    #[error("duplicate PostgreSQL URL query option")]
    DuplicateUrlOption,
    #[error("PostgreSQL URL conflicts with protected TLS/application policy")]
    UrlPolicyMismatch,
    #[error("failed to read PostgreSQL configuration file for {input}")]
    FileRead {
        input: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("PostgreSQL configuration file must be regular: {0}")]
    NotRegularFile(&'static str),
    #[error("PostgreSQL configuration file exceeds size limit: {0}")]
    FileTooLarge(&'static str),
    #[error("PostgreSQL password file requires Unix mode 0400 or 0600")]
    SecretFilePermissions,
}

impl fmt::Debug for PostgresPoolConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Debug, Display, and every exposed source-chain member redact provider data.
#[derive(thiserror::Error)]
#[error("failed to connect to Postgres")]
pub struct PostgresConnectError {
    #[source]
    source: RedactedPostgresConnectCause,
}

// Keep the original cause, but never expose it through Error::source or an accessor.
struct RedactedPostgresConnectCause {
    original: sqlx::Error,
}

impl RedactedPostgresConnectCause {
    fn classification(&self) -> &'static str {
        match &self.original {
            sqlx::Error::Configuration(_) => "PostgreSQL connection configuration rejected",
            sqlx::Error::Database(error) => match error.code().as_deref() {
                Some("28P01" | "28000") => "PostgreSQL authentication rejected",
                _ => "PostgreSQL server rejected connection",
            },
            sqlx::Error::Tls(_) => "PostgreSQL TLS negotiation failed",
            sqlx::Error::Io(error) if error.kind() == io::ErrorKind::TimedOut => {
                "PostgreSQL connection timed out"
            }
            sqlx::Error::Io(_) => "PostgreSQL connection I/O failed",
            sqlx::Error::PoolTimedOut => "PostgreSQL connection timed out",
            sqlx::Error::PoolClosed => "PostgreSQL connection pool closed",
            _ => "PostgreSQL connection protocol failed",
        }
    }
}

impl fmt::Display for RedactedPostgresConnectCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.classification())
    }
}

impl fmt::Debug for RedactedPostgresConnectCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedactedPostgresConnectCause")
            .field("classification", &self.classification())
            .finish_non_exhaustive()
    }
}

impl std::error::Error for RedactedPostgresConnectCause {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

impl From<sqlx::Error> for PostgresConnectError {
    fn from(original: sqlx::Error) -> Self {
        Self {
            source: RedactedPostgresConnectCause { original },
        }
    }
}

impl fmt::Debug for PostgresConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PostgresConnectError")
            .field("source", &self.source)
            .finish()
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
