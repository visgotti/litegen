//! Where litegen finds its database and its Redis: settings that name a
//! connection, resolved once at startup (`docs/scaling.md`, "Connection
//! settings").
//!
//! Both have a whole-URL form and a parts form, so a managed service's host,
//! port and password can be set as they are handed out. A URL always wins over
//! the parts, and a setting that loses to a higher one is named in a startup
//! warning. Warnings and errors name variables, never their values.

use std::borrow::Cow;
use std::collections::HashMap;
use std::net::Ipv6Addr;
use std::path::PathBuf;
use std::time::Duration;

/// Percent-encode a user, password, database name or query value. Only
/// RFC 3986's unreserved characters (letters, digits, `-`, `.`, `_`, `~`) stay
/// as they are, so a password with `@`, `:` or `/` in it survives, and
/// `/etc/ca.crt` is `%2Fetc%2Fca.crt`.
fn encode(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

// ── Database: each setting under both spellings ─────────────────────────────

const DB_URL: &[&str] = &["LITEGEN_DATABASE_URL", "LITEGEN_DB_URL"];
const DB_HOST: &[&str] = &["LITEGEN_DATABASE_HOST", "LITEGEN_DB_HOST"];
const DB_PORT: &[&str] = &["LITEGEN_DATABASE_PORT", "LITEGEN_DB_PORT"];
const DB_USER: &[&str] = &["LITEGEN_DATABASE_USER", "LITEGEN_DATABASE_USERNAME", "LITEGEN_DB_USER", "LITEGEN_DB_USERNAME"];
const DB_PASSWORD: &[&str] = &["LITEGEN_DATABASE_PASSWORD", "LITEGEN_DB_PASSWORD"];
const DB_NAME: &[&str] = &["LITEGEN_DATABASE_NAME", "LITEGEN_DB_NAME"];
const DB_SSLMODE: &[&str] = &["LITEGEN_DATABASE_SSLMODE", "LITEGEN_DB_SSLMODE"];
const DB_SSLROOTCERT: &[&str] = &["LITEGEN_DATABASE_SSLROOTCERT", "LITEGEN_DB_SSLROOTCERT"];
/// The settings that build a URL when there is none.
const DB_PARTS: &[&[&str]] = &[DB_HOST, DB_PORT, DB_USER, DB_PASSWORD, DB_NAME, DB_SSLMODE, DB_SSLROOTCERT];
/// The double-underscore name config-rs reads into `database_url`. It keeps
/// working, and ranks below the new URL names and above the parts.
const LEGACY_DB_URL: &str = "LITEGEN__DATABASE_URL";
const DEFAULT_DB_PORT: u16 = 5432;

// ── Redis ───────────────────────────────────────────────────────────────────

const REDIS_URL: &str = "LITEGEN_REDIS_URL";
const REDIS_HOST: &str = "LITEGEN_REDIS_HOST";
const REDIS_PORT: &str = "LITEGEN_REDIS_PORT";
const REDIS_USERNAME: &str = "LITEGEN_REDIS_USERNAME";
const REDIS_PASSWORD: &str = "LITEGEN_REDIS_PASSWORD";
const REDIS_SSL: &str = "LITEGEN_REDIS_SSL";
const REDIS_SSL_CA_CERTS: &str = "LITEGEN_REDIS_SSL_CA_CERTS";
const REDIS_SSL_CERT_REQS: &str = "LITEGEN_REDIS_SSL_CERT_REQS";
const REDIS_BREAKER_THRESHOLD: &str = "LITEGEN_REDIS_CIRCUIT_BREAKER_FAILURE_THRESHOLD";
const REDIS_BREAKER_RECOVERY: &str = "LITEGEN_REDIS_CIRCUIT_BREAKER_RECOVERY_TIMEOUT";
const REDIS_SLOW_CALL: &str = "LITEGEN_REDIS_SLOW_CALL_MS";
const FAIL_CLOSED: &str = "LITEGEN_FAIL_CLOSED_RATE_LIMIT_ENFORCEMENT";
/// The parts that a `LITEGEN_REDIS_URL` makes pointless. The two TLS settings
/// are not among them: they apply to a URL as well.
const REDIS_PARTS: &[&str] = &[REDIS_HOST, REDIS_PORT, REDIS_USERNAME, REDIS_PASSWORD, REDIS_SSL];
/// Everything that only means something next to a URL or a host. Set without
/// either, it is named in a warning and Redis stays off.
const REDIS_COMPANIONS: &[&str] = &[REDIS_PORT, REDIS_USERNAME, REDIS_PASSWORD, REDIS_SSL, REDIS_SSL_CA_CERTS, REDIS_SSL_CERT_REQS];
const DEFAULT_REDIS_PORT: u16 = 6379;
/// The port variables a Kubernetes Service link can set: a Service named
/// `litegen-db`, `litegen-database` or `litegen-redis` puts `tcp://ip:port` in
/// `LITEGEN_DB_PORT`, `LITEGEN_DATABASE_PORT` or `LITEGEN_REDIS_PORT` of every
/// pod in its namespace.
const SERVICE_LINK_PORTS: &[&str] = &[DB_PORT[0], DB_PORT[1], REDIS_PORT];

/// How to reach Redis, and what to do when it can't be reached.
#[derive(Clone)]
pub struct RedisSettings {
    /// A PEM file of CA certificates to trust besides the system's roots.
    pub ca_certs: Option<PathBuf>,
    /// Check the server's certificate and host name (the default).
    pub verify_certificates: bool,
    /// Failures in a row that open the Redis breaker.
    pub breaker_failure_threshold: u32,
    /// How long the breaker stays open before it probes again.
    pub breaker_recovery: Duration,
    /// An answer Redis takes this long or longer to give counts as a breaker
    /// failure (`LITEGEN_REDIS_SLOW_CALL_MS`, 250 ms by default). Zero turns
    /// that off.
    pub slow_call: Duration,
    /// Refuse limited requests with 503 while Redis can't be reached.
    pub fail_closed: bool,
    /// May hold a password: private, and left out of `Debug`.
    url: String,
}

impl RedisSettings {
    /// The settings for `url` with the defaults: certificates verified, a
    /// breaker that opens after 5 failures and probes after 60 s, an answer
    /// of 250 ms or more counted as a failure, and limits that stay open
    /// (fail-open) while Redis is down.
    pub fn from_url(url: &str) -> RedisSettings {
        RedisSettings {
            ca_certs: None,
            verify_certificates: true,
            breaker_failure_threshold: 5,
            breaker_recovery: Duration::from_secs(60),
            slow_call: Duration::from_millis(250),
            fail_closed: false,
            url: url.to_string(),
        }
    }

    /// The URL to connect to. It may hold a password: keep it out of logs.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// `host:port`, with no credentials, to say in logs which Redis this is.
    /// The port is 6379 when the URL names none. A URL that does not have the
    /// shape `scheme://[user:password@]host[:port]` gives a fixed placeholder,
    /// with none of its text, since any of it might be a password.
    pub fn target(&self) -> String {
        url_target(&self.url).unwrap_or_else(|| UNPARSABLE_URL.to_string())
    }
}

/// What `target()` says when it can't tell the host from the credentials.
const UNPARSABLE_URL: &str = "<unparsable Redis URL>";

/// The `host:port` of a Redis URL: a host name or address, or an IPv6 address
/// in brackets, and a numeric port (6379 when there is none). `None` for
/// anything else: no `://`, an empty host, a port that isn't digits, a bracket
/// that isn't IPv6.
fn url_target(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    // Credentials end at the last `@` of everything after the scheme, and
    // only then is the host cut at a path or query. Cutting there first
    // means a password with an unencoded `/`, `?`, `#` or `@` in it can't
    // be left over.
    let after_credentials = rest.rsplit_once('@').map_or(rest, |(_, hostport)| hostport);
    let hostport = after_credentials.split(['/', '?', '#']).next()?;
    let (host, port) = match hostport.strip_prefix('[') {
        Some(bracketed) => {
            let (inner, after) = bracketed.split_once(']')?;
            inner.parse::<Ipv6Addr>().ok()?;
            (format!("[{inner}]"), after)
        }
        None => {
            let (host, port) = hostport.split_at(hostport.find(':').unwrap_or(hostport.len()));
            (host.to_string(), port)
        }
    };
    if host.is_empty() {
        return None;
    }
    let port = match port.strip_prefix(':') {
        None if port.is_empty() => DEFAULT_REDIS_PORT.to_string(),
        Some(digits) if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) => digits.to_string(),
        _ => return None,
    };
    Some(format!("{host}:{port}"))
}

/// Only `target()`: the URL may hold a password.
impl std::fmt::Debug for RedisSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisSettings").field("target", &self.target()).finish()
    }
}

/// The connections litegen starts with.
pub struct Connections {
    /// The database URL to open: Postgres, or SQLite as before.
    pub database_url: String,
    /// `None` when neither `LITEGEN_REDIS_URL` nor `LITEGEN_REDIS_HOST` is set.
    pub redis: Option<RedisSettings>,
    /// One line per setting that lost to a higher one; `main` logs each.
    pub warnings: Vec<String>,
    /// No setting names a database, so `database_url` is the built-in
    /// default ([`default_database_in`]).
    pub database_is_default: bool,
}

/// Without `database_url`: the URL may hold a password.
impl std::fmt::Debug for Connections {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connections")
            .field("redis", &self.redis)
            .field("warnings", &self.warnings)
            .finish_non_exhaustive()
    }
}

/// Resolve the database URL and Redis from the environment `env`.
/// `configured_database_url` is what config-rs produced for `database_url`:
/// the `LITEGEN__DATABASE_URL` variable, else the config file (named
/// `config_file` in warnings), else the default. An `Err` is the message that
/// names the variables involved, never their values.
pub fn resolve_connections(env: &HashMap<String, String>, configured_database_url: &str, config_file: Option<&str>) -> Result<Connections, String> {
    let mut warnings = Vec::new();
    let env = without_service_links(env, &mut warnings);
    let file_url = format!("database_url in {}", config_file.unwrap_or("the config file"));
    let named = resolve_database(&env, configured_database_url, &file_url, &mut warnings)?;
    let database_is_default = named.is_none() && configured_database_url == super::DEFAULT_DATABASE_URL;
    let database_url = named.unwrap_or_else(|| configured_database_url.to_string());
    let redis = resolve_redis(&env, &mut warnings)?;
    Ok(Connections { database_url, redis, warnings, database_is_default })
}

/// The database to open in `cwd` when no setting names one, and a warning for
/// the startup log: `data/litegen.db` ([`super::DEFAULT_DATABASE_URL`]).
///
/// The old default was `./litegen.db`, which SQLite would only open, never
/// create, so anyone who ran on it made that file by hand. Until
/// `data/litegen.db` exists, such a file stays the database, with a warning:
/// a silent move to a new, empty database would look like lost keys and users.
pub fn default_database_in(cwd: &std::path::Path) -> (String, Option<String>) {
    let default = super::DEFAULT_DATABASE_URL.to_string();
    match (cwd.join("litegen.db").is_file(), cwd.join("data/litegen.db").exists()) {
        (true, false) => (
            "sqlite://litegen.db".to_string(),
            Some(
                "litegen.db in the working directory is used, as before, but the default database is now data/litegen.db: \
                 stop litegen and move litegen.db there (with any litegen.db-journal beside it), \
                 or set LITEGEN_DATABASE_URL=sqlite://litegen.db to keep it where it is"
                    .to_string(),
            ),
        ),
        (true, true) => (
            default,
            Some(
                "litegen.db in the working directory is not used: the database is data/litegen.db. \
                 Set LITEGEN_DATABASE_URL=sqlite://litegen.db to use the old file instead"
                    .to_string(),
            ),
        ),
        _ => (default, None),
    }
}

// ── Lookups ─────────────────────────────────────────────────────────────────

/// "a", "a and b", "a, b and c", with `joiner` "and" or "or".
pub(super) fn list(names: &[&str], joiner: &str) -> String {
    match names {
        [] => String::new(),
        [only] => only.to_string(),
        [init @ .., last] => format!("{} {joiner} {last}", init.join(", ")),
    }
}

/// "is" for one, "are" for several.
fn is_are(count: usize) -> &'static str {
    if count == 1 { "is" } else { "are" }
}

/// "X is ignored because Y is set", with "are" for several.
pub(super) fn ignored(names: &[&str], winner: &str) -> String {
    format!("{} {} ignored because {winner} is set", list(names, "and"), is_are(names.len()))
}

/// `env` without the Kubernetes Service links among the port variables. A
/// Service named `litegen-db`, `litegen-database` or `litegen-redis` makes
/// Kubernetes put `tcp://10.0.0.5:5432` in `LITEGEN_DB_PORT`,
/// `LITEGEN_DATABASE_PORT` or `LITEGEN_REDIS_PORT` of every pod, by default.
/// Those are no port of ours: each is dropped, with a warning that names it.
/// They therefore count as neither a stray part nor an ignored one.
fn without_service_links<'a>(env: &'a HashMap<String, String>, warnings: &mut Vec<String>) -> Cow<'a, HashMap<String, String>> {
    let links: Vec<&str> = SERVICE_LINK_PORTS.iter().copied().filter(|n| env.get(*n).is_some_and(|v| v.trim_start().starts_with("tcp://"))).collect();
    if links.is_empty() {
        return Cow::Borrowed(env);
    }
    let mut env = env.clone();
    for name in links {
        env.remove(name);
        warnings.push(format!(
            "{name} looks like a Kubernetes service link (its value starts with tcp://) and is ignored; \
             enableServiceLinks: false on the pod, or another name for the Service, stops Kubernetes setting it"
        ));
    }
    Cow::Owned(env)
}

/// A host for a URL. An IPv6 address gets its brackets; anything else with a
/// port, path, credentials, brackets or space in it is refused, naming the variable
/// (`name`) and the ones that hold the port (`port_names`), never the value.
fn url_host(name: &str, host: &str, port_names: &[&str]) -> Result<String, String> {
    let host = host.trim();
    let inner = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    if inner.parse::<Ipv6Addr>().is_ok() {
        return Ok(format!("[{inner}]"));
    }
    if host.contains(|c: char| matches!(c, ':' | '/' | '@' | '?' | '#' | '[' | ']') || c.is_whitespace()) {
        return Err(format!("{name} must be a host name or address alone; put the port in {}", list(port_names, "or")));
    }
    Ok(host.to_string())
}

/// A variable's value. Unset, empty and blank are the same.
fn get<'a>(env: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    env.get(name).map(String::as_str).filter(|v| !v.trim().is_empty())
}

/// The names of `names` that are set.
fn set_names(env: &HashMap<String, String>, names: &[&'static str]) -> Vec<&'static str> {
    names.iter().copied().filter(|n| get(env, n).is_some()).collect()
}

/// A setting that has several names: its name and value, from the first of
/// them that is set. Two of them set to different values is an error.
fn setting<'a>(env: &'a HashMap<String, String>, names: &[&'static str]) -> Result<Option<(&'static str, &'a str)>, String> {
    let set: Vec<(&'static str, &'a str)> = names.iter().filter_map(|&n| get(env, n).map(|v| (n, v))).collect();
    let Some(&first) = set.first() else { return Ok(None) };
    if set.iter().any(|(_, v)| *v != first.1) {
        let clashing: Vec<&str> = set.iter().map(|(n, _)| *n).collect();
        return Err(format!("{} are set to different values; set only one of them", list(&clashing, "and")));
    }
    Ok(Some(first))
}

fn parse_port(name: &str, value: &str) -> Result<u16, String> {
    value.trim().parse::<u16>().ok().filter(|p| *p != 0).ok_or_else(|| format!("{name} must be a port number, 1 to 65535"))
}

fn whole_number<T: std::str::FromStr>(name: &str, value: &str) -> Result<T, String> {
    value.trim().parse::<T>().map_err(|_| format!("{name} must be a whole number"))
}

fn parse_bool(name: &str, value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(format!("{name} must be true or false")),
    }
}

// ── Database ────────────────────────────────────────────────────────────────

/// The database URL the variables give: the URL, the legacy variable's (which
/// config-rs put in `configured`), or one built from the parts. `None` when no
/// variable names a database, so the configured URL (the file's or the
/// default) stands. `file_url` names the config file's `database_url` in
/// warnings.
fn resolve_database(env: &HashMap<String, String>, configured: &str, file_url: &str, warnings: &mut Vec<String>) -> Result<Option<String>, String> {
    // Every setting is read first, so a clash between its spellings is
    // refused whichever setting wins.
    let new_url = setting(env, DB_URL)?;
    let host = setting(env, DB_HOST)?;
    let port = setting(env, DB_PORT)?;
    let user = setting(env, DB_USER)?;
    let password = setting(env, DB_PASSWORD)?;
    let name = setting(env, DB_NAME)?;
    let sslmode = setting(env, DB_SSLMODE)?;
    let sslrootcert = setting(env, DB_SSLROOTCERT)?;
    let legacy_url = get(env, LEGACY_DB_URL).is_some();
    let parts_set: Vec<&str> = DB_PARTS.iter().flat_map(|names| set_names(env, names)).collect();
    // The yaml file's URL can't be told from the default unless it differs.
    let yaml_url = !legacy_url && configured != super::DEFAULT_DATABASE_URL;

    if let Some((winner, url)) = new_url {
        if legacy_url {
            warnings.push(ignored(&[LEGACY_DB_URL], winner));
        }
        if !parts_set.is_empty() {
            warnings.push(ignored(&parts_set, winner));
        }
        if yaml_url {
            warnings.push(ignored(&[file_url], winner));
        }
        return Ok(Some(url.trim().to_string()));
    }
    if legacy_url {
        if !parts_set.is_empty() {
            warnings.push(ignored(&parts_set, LEGACY_DB_URL));
        }
        return Ok(Some(configured.to_string()));
    }
    let Some((host_var, host)) = host else {
        // Parts with no host: the host is what turns parts mode on, so these
        // would be dropped and litegen would start on the configured URL
        // (SQLite by default) without a word. Refuse, as for a missing user.
        if !parts_set.is_empty() {
            return Err(format!(
                "{} {} set, but the database host is not: set {} (or {}), or set {} instead",
                list(&parts_set, "and"), is_are(parts_set.len()), DB_HOST[0], DB_HOST[1], DB_URL[0]
            ));
        }
        return Ok(None);
    };

    // Parts mode: a host without a user and a database name would otherwise
    // fall back to SQLite without a word.
    // The user, name and TLS values are trimmed; a password never is.
    let needed = |names: &[&str]| format!("{host_var} is set, so litegen builds the database URL from its parts, and that needs {}", list(names, "or"));
    let user = user.ok_or_else(|| needed(DB_USER))?.1.trim();
    let name = name.ok_or_else(|| needed(DB_NAME))?.1.trim();
    let port = match port {
        Some((var, value)) => parse_port(var, value)?,
        None => DEFAULT_DB_PORT,
    };
    let mut url = format!("postgres://{}", encode(user));
    if let Some((_, password)) = password {
        url.push(':');
        url.push_str(&encode(password));
    }
    url.push_str(&format!("@{}:{port}/{}", url_host(host_var, host, DB_PORT)?, encode(name)));
    let mut query = Vec::new();
    for (key, value) in [("sslmode", sslmode), ("sslrootcert", sslrootcert)] {
        if let Some((_, value)) = value {
            query.push(format!("{key}={}", encode(value.trim())));
        }
    }
    if !query.is_empty() {
        url.push('?');
        url.push_str(&query.join("&"));
    }
    if yaml_url {
        warnings.push(ignored(&[file_url], host_var));
    }
    Ok(Some(url))
}

// ── Redis ───────────────────────────────────────────────────────────────────

fn resolve_redis(env: &HashMap<String, String>, warnings: &mut Vec<String>) -> Result<Option<RedisSettings>, String> {
    let url = match (get(env, REDIS_URL), get(env, REDIS_HOST)) {
        (None, None) => {
            let stray: Vec<&str> = REDIS_COMPANIONS.iter().copied().filter(|n| get(env, n).is_some()).collect();
            if !stray.is_empty() {
                warnings.push(format!("{} {} set, but neither {REDIS_URL} nor {REDIS_HOST} is, so Redis is off", list(&stray, "and"), is_are(stray.len())));
            }
            return Ok(None);
        }
        (Some(url), _) => {
            let url = url.trim();
            if !(url.starts_with("redis://") || url.starts_with("rediss://")) {
                return Err(format!("{REDIS_URL} must start with redis:// or rediss://"));
            }
            // Userinfo with no `@host` would otherwise read as a host and a port.
            if url_target(url).is_none() {
                return Err(format!("{REDIS_URL} is not a valid Redis URL: it needs redis://[user:password@]host[:port]"));
            }
            let ignored_parts: Vec<&str> = REDIS_PARTS.iter().copied().filter(|n| get(env, n).is_some()).collect();
            if !ignored_parts.is_empty() {
                warnings.push(ignored(&ignored_parts, REDIS_URL));
            }
            url.to_string()
        }
        (None, Some(host)) => {
            let port = match get(env, REDIS_PORT) {
                Some(value) => parse_port(REDIS_PORT, value)?,
                None => DEFAULT_REDIS_PORT,
            };
            let ssl = match get(env, REDIS_SSL) {
                Some(value) => parse_bool(REDIS_SSL, value)?,
                None => false,
            };
            let scheme = if ssl { "rediss" } else { "redis" };
            let credentials = match (get(env, REDIS_USERNAME).map(str::trim), get(env, REDIS_PASSWORD)) {
                (None, None) => String::new(),
                (Some(user), None) => format!("{}@", encode(user)),
                (None, Some(password)) => format!(":{}@", encode(password)),
                (Some(user), Some(password)) => format!("{}:{}@", encode(user), encode(password)),
            };
            format!("{scheme}://{credentials}{}:{port}", url_host(REDIS_HOST, host, &[REDIS_PORT])?)
        }
    };

    let mut settings = RedisSettings::from_url(&url);
    settings.ca_certs = get(env, REDIS_SSL_CA_CERTS).map(|p| PathBuf::from(p.trim()));
    if let Some(value) = get(env, REDIS_SSL_CERT_REQS) {
        settings.verify_certificates = match value.trim().to_ascii_lowercase().as_str() {
            "none" => false,
            "required" => true,
            _ => return Err(format!("{REDIS_SSL_CERT_REQS} must be none or required")),
        };
    }
    if let Some(value) = get(env, REDIS_BREAKER_THRESHOLD) {
        settings.breaker_failure_threshold = whole_number(REDIS_BREAKER_THRESHOLD, value)?;
        if settings.breaker_failure_threshold == 0 {
            return Err(format!("{REDIS_BREAKER_THRESHOLD} must be at least 1"));
        }
    }
    if let Some(value) = get(env, REDIS_BREAKER_RECOVERY) {
        settings.breaker_recovery = Duration::from_secs(whole_number(REDIS_BREAKER_RECOVERY, value)?);
    }
    if let Some(value) = get(env, REDIS_SLOW_CALL) {
        settings.slow_call = Duration::from_millis(whole_number(REDIS_SLOW_CALL, value)?);
    }
    if let Some(value) = get(env, FAIL_CLOSED) {
        settings.fail_closed = parse_bool(FAIL_CLOSED, value)?;
    }
    Ok(Some(settings))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> { pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }
    const DEFAULT: &str = crate::config::DEFAULT_DATABASE_URL;
    /// As litegen resolves them with a `litegen.yaml` found in its directory.
    fn resolve_connections(env: &HashMap<String, String>, configured_database_url: &str) -> Result<Connections, String> {
        super::resolve_connections(env, configured_database_url, Some("litegen.yaml"))
    }

    #[test] fn no_redis_variables_means_no_redis() {
        assert!(resolve_connections(&env(&[]), DEFAULT).unwrap().redis.is_none());
    }
    #[test] fn redis_parts_build_a_url_with_default_port() {
        let c = resolve_connections(&env(&[("LITEGEN_REDIS_HOST", "cache.internal")]), DEFAULT).unwrap();
        assert_eq!(c.redis.unwrap().url(), "redis://cache.internal:6379");
    }
    #[test] fn redis_parts_percent_encode_username_and_password() {
        let c = resolve_connections(&env(&[("LITEGEN_REDIS_HOST", "h"), ("LITEGEN_REDIS_PORT", "25061"),
            ("LITEGEN_REDIS_USERNAME", "default"), ("LITEGEN_REDIS_PASSWORD", "p@ss:w/rd%"), ("LITEGEN_REDIS_SSL", "true")]), DEFAULT).unwrap();
        assert_eq!(c.redis.unwrap().url(), "rediss://default:p%40ss%3Aw%2Frd%25@h:25061");
    }
    #[test] fn redis_url_wins_over_parts_and_warns() {
        let c = resolve_connections(&env(&[("LITEGEN_REDIS_URL", "redis://a:6379"), ("LITEGEN_REDIS_HOST", "b")]), DEFAULT).unwrap();
        assert_eq!(c.redis.unwrap().url(), "redis://a:6379");
        assert!(c.warnings.iter().any(|w| w.contains("LITEGEN_REDIS_HOST") && w.contains("LITEGEN_REDIS_URL")));
    }
    #[test] fn redis_cert_reqs_none_turns_verification_off() {
        let c = resolve_connections(&env(&[("LITEGEN_REDIS_URL", "rediss://a:1"), ("LITEGEN_REDIS_SSL_CERT_REQS", "none")]), DEFAULT).unwrap();
        assert!(!c.redis.unwrap().verify_certificates);
        assert!(resolve_connections(&env(&[("LITEGEN_REDIS_URL", "rediss://a:1"), ("LITEGEN_REDIS_SSL_CERT_REQS", "maybe")]), DEFAULT)
            .unwrap_err().contains("LITEGEN_REDIS_SSL_CERT_REQS"));
    }
    #[test] fn outage_settings_parse_with_defaults() {
        let r = resolve_connections(&env(&[("LITEGEN_REDIS_HOST", "h")]), DEFAULT).unwrap().redis.unwrap();
        assert_eq!((r.breaker_failure_threshold, r.breaker_recovery, r.fail_closed), (5, Duration::from_secs(60), false));
        let r = resolve_connections(&env(&[("LITEGEN_REDIS_HOST", "h"), ("LITEGEN_FAIL_CLOSED_RATE_LIMIT_ENFORCEMENT", "true"),
            ("LITEGEN_REDIS_CIRCUIT_BREAKER_FAILURE_THRESHOLD", "3"), ("LITEGEN_REDIS_CIRCUIT_BREAKER_RECOVERY_TIMEOUT", "10")]), DEFAULT).unwrap().redis.unwrap();
        assert_eq!((r.breaker_failure_threshold, r.breaker_recovery, r.fail_closed), (3, Duration::from_secs(10), true));
    }
    #[test] fn the_slow_call_setting_is_in_milliseconds_and_zero_turns_it_off() {
        let slow = |pairs: &[(&str, &str)]| resolve_connections(&env(pairs), DEFAULT).map(|c| c.redis.unwrap().slow_call);
        assert_eq!(slow(&[("LITEGEN_REDIS_HOST", "h")]), Ok(Duration::from_millis(250)), "the default");
        assert_eq!(slow(&[("LITEGEN_REDIS_HOST", "h"), ("LITEGEN_REDIS_SLOW_CALL_MS", " 100 ")]), Ok(Duration::from_millis(100)));
        assert_eq!(slow(&[("LITEGEN_REDIS_HOST", "h"), ("LITEGEN_REDIS_SLOW_CALL_MS", "0")]), Ok(Duration::ZERO), "0 turns it off");
        for bad in ["fast", "-1", "0.5", "250ms"] {
            let e = slow(&[("LITEGEN_REDIS_HOST", "h"), ("LITEGEN_REDIS_SLOW_CALL_MS", bad)]).unwrap_err();
            assert!(e.contains("LITEGEN_REDIS_SLOW_CALL_MS") && e.contains("whole number"), "{bad}: {e}");
        }
    }
    #[test] fn database_url_new_names_win() {
        for name in ["LITEGEN_DATABASE_URL", "LITEGEN_DB_URL"] {
            let c = resolve_connections(&env(&[(name, "postgres://x/y"), ("LITEGEN__DATABASE_URL", "postgres://old/db")]), "postgres://old/db").unwrap();
            assert_eq!(c.database_url, "postgres://x/y");
            assert!(c.warnings.iter().any(|w| w.contains("LITEGEN__DATABASE_URL")));
        }
    }
    #[test] fn database_and_db_spellings_that_differ_refuse_to_start() {
        let e = resolve_connections(&env(&[("LITEGEN_DATABASE_HOST", "a"), ("LITEGEN_DB_HOST", "b")]), DEFAULT).unwrap_err();
        assert!(e.contains("LITEGEN_DATABASE_HOST") && e.contains("LITEGEN_DB_HOST"));
    }
    #[test] fn database_and_db_spellings_that_match_are_fine() {
        let c = resolve_connections(&env(&[("LITEGEN_DATABASE_URL", "postgres://x/y"), ("LITEGEN_DB_URL", "postgres://x/y")]), DEFAULT).unwrap();
        assert_eq!(c.database_url, "postgres://x/y");
    }
    #[test] fn legacy_double_underscore_url_beats_parts_and_warns() {
        let c = resolve_connections(&env(&[("LITEGEN__DATABASE_URL", "postgres://old/db"), ("LITEGEN_DB_HOST", "h"),
            ("LITEGEN_DB_USER", "u"), ("LITEGEN_DB_NAME", "n")]), "postgres://old/db").unwrap();
        assert_eq!(c.database_url, "postgres://old/db");
        assert!(c.warnings.iter().any(|w| w.contains("LITEGEN_DB_HOST")));
    }
    #[test] fn database_parts_build_a_postgres_url() {
        let c = resolve_connections(&env(&[("LITEGEN_DB_HOST", "db.internal"), ("LITEGEN_DB_USERNAME", "lite gen"),
            ("LITEGEN_DATABASE_PASSWORD", "p@ss"), ("LITEGEN_DB_NAME", "litegen"), ("LITEGEN_DB_SSLMODE", "verify-full"),
            ("LITEGEN_DB_SSLROOTCERT", "/etc/ca.crt")]), DEFAULT).unwrap();
        assert_eq!(c.database_url, "postgres://lite%20gen:p%40ss@db.internal:5432/litegen?sslmode=verify-full&sslrootcert=%2Fetc%2Fca.crt");
    }
    #[test] fn database_parts_missing_user_or_name_refuse_to_start() {
        assert!(resolve_connections(&env(&[("LITEGEN_DB_HOST", "h"), ("LITEGEN_DB_NAME", "n")]), DEFAULT).unwrap_err().contains("LITEGEN_DATABASE_USER"));
        assert!(resolve_connections(&env(&[("LITEGEN_DB_HOST", "h"), ("LITEGEN_DB_USER", "u")]), DEFAULT).unwrap_err().contains("LITEGEN_DATABASE_NAME"));
    }
    #[test] fn the_configured_url_is_kept_when_nothing_is_set() {
        assert_eq!(resolve_connections(&env(&[]), "sqlite://from-yaml.db").unwrap().database_url, "sqlite://from-yaml.db");
    }
    #[test] fn database_parts_beat_the_yaml_url_and_a_new_url_beats_parts() {
        let parts = [("LITEGEN_DB_HOST", "h"), ("LITEGEN_DB_USER", "u"), ("LITEGEN_DB_NAME", "n")];
        assert_eq!(resolve_connections(&env(&parts), "sqlite://from-yaml.db").unwrap().database_url, "postgres://u@h:5432/n", "no password: no colon");
        let mut with_url = parts.to_vec(); with_url.push(("LITEGEN_DATABASE_URL", "postgres://x/y"));
        let c = resolve_connections(&env(&with_url), DEFAULT).unwrap();
        assert_eq!(c.database_url, "postgres://x/y");
        assert!(c.warnings.iter().any(|w| w.contains("LITEGEN_DB_HOST") && w.contains("LITEGEN_DATABASE_URL")));
    }
    #[test] fn redis_ca_certs_path_is_kept() {
        let r = resolve_connections(&env(&[("LITEGEN_REDIS_URL", "rediss://a:1"), ("LITEGEN_REDIS_SSL_CA_CERTS", "/etc/redis-ca.pem")]), DEFAULT).unwrap().redis.unwrap();
        assert_eq!(r.ca_certs.as_deref(), Some(std::path::Path::new("/etc/redis-ca.pem")));
        assert!(r.verify_certificates, "checks stay on by default");
    }
    #[test] fn debug_output_never_shows_a_password() {
        let r = RedisSettings::from_url("redis://u:hunter2@h:6379");
        assert!(!format!("{r:?}").contains("hunter2"));
        let e = resolve_connections(&env(&[("LITEGEN_DATABASE_PASSWORD", "hunter2"), ("LITEGEN_DB_PASSWORD", "other"), ("LITEGEN_DB_HOST", "h")]), DEFAULT).unwrap_err();
        assert!(!e.contains("hunter2") && !e.contains("other"));
    }

    // ── Beyond the brief's list: the behavior the resolver adds around it ──

    #[test] fn target_leaves_out_credentials_and_defaults_the_port() {
        assert_eq!(RedisSettings::from_url("rediss://default:p%40ss@cache.internal:25061/0").target(), "cache.internal:25061");
        assert_eq!(RedisSettings::from_url("redis://:hunter2@cache.internal").target(), "cache.internal:6379");
        assert_eq!(RedisSettings::from_url("redis://cache.internal:6379?x=y").target(), "cache.internal:6379");
        // A password with a `/` the user forgot to encode must not be left over.
        let t = RedisSettings::from_url("redis://u:hun/ter2@h:6379").target();
        assert_eq!(t, "h:6379");
        // Nor may any other character that ends something in a URL, unencoded.
        for pw in ["a#b", "a?b", "a/b", "a@b", "a:b", "a%b", "a#b?c/d@e:f%g"] {
            assert_eq!(RedisSettings::from_url(&format!("redis://default:{pw}@host:6379")).target(), "host:6379", "{pw}");
            assert_eq!(RedisSettings::from_url(&format!("redis://default:{pw}@host:25061/0")).target(), "host:25061", "{pw}");
            assert_eq!(RedisSettings::from_url(&format!("redis://default:{pw}@host")).target(), "host:6379", "{pw}");
            assert_eq!(RedisSettings::from_url(&format!("redis://:{pw}@host:6379?x=y")).target(), "host:6379", "{pw}");
        }
    }
    #[test] fn connections_debug_leaves_out_the_database_url() {
        let c = resolve_connections(&env(&[("LITEGEN_DATABASE_URL", "postgres://u:hunter2@h/db")]), DEFAULT).unwrap();
        assert!(!format!("{c:?}").contains("hunter2"));
    }
    #[test] fn blank_values_count_as_unset() {
        let c = resolve_connections(&env(&[("LITEGEN_REDIS_HOST", ""), ("LITEGEN_REDIS_URL", "  "), ("LITEGEN_DB_HOST", ""), ("LITEGEN_DATABASE_HOST", "")]), "sqlite://x.db").unwrap();
        assert!(c.redis.is_none());
        assert_eq!(c.database_url, "sqlite://x.db");
        assert!(c.warnings.is_empty());
    }
    #[test] fn bad_ports_urls_and_numbers_name_the_variable() {
        let bad = |pairs: &[(&str, &str)]| resolve_connections(&env(pairs), DEFAULT).unwrap_err();
        assert!(bad(&[("LITEGEN_REDIS_HOST", "h"), ("LITEGEN_REDIS_PORT", "http")]).contains("LITEGEN_REDIS_PORT"));
        assert!(bad(&[("LITEGEN_REDIS_URL", "http://h:1")]).contains("LITEGEN_REDIS_URL"));
        assert!(bad(&[("LITEGEN_REDIS_HOST", "h"), ("LITEGEN_REDIS_SSL", "maybe")]).contains("LITEGEN_REDIS_SSL"));
        assert!(bad(&[("LITEGEN_REDIS_HOST", "h"), ("LITEGEN_REDIS_CIRCUIT_BREAKER_FAILURE_THRESHOLD", "many")]).contains("LITEGEN_REDIS_CIRCUIT_BREAKER_FAILURE_THRESHOLD"));
        assert!(bad(&[("LITEGEN_REDIS_HOST", "h"), ("LITEGEN_FAIL_CLOSED_RATE_LIMIT_ENFORCEMENT", "sometimes")]).contains("LITEGEN_FAIL_CLOSED_RATE_LIMIT_ENFORCEMENT"));
        assert!(bad(&[("LITEGEN_DB_HOST", "h"), ("LITEGEN_DB_USER", "u"), ("LITEGEN_DB_NAME", "n"), ("LITEGEN_DB_PORT", "99999")]).contains("LITEGEN_DB_PORT"));
    }
    #[test] fn redis_url_with_only_a_username_or_only_a_password() {
        let url = |pairs: &[(&str, &str)]| resolve_connections(&env(pairs), DEFAULT).unwrap().redis.unwrap().url().to_string();
        assert_eq!(url(&[("LITEGEN_REDIS_HOST", "h"), ("LITEGEN_REDIS_USERNAME", "u")]), "redis://u@h:6379");
        assert_eq!(url(&[("LITEGEN_REDIS_HOST", "h"), ("LITEGEN_REDIS_PASSWORD", "pw")]), "redis://:pw@h:6379");
    }
    #[test] fn a_yaml_url_that_loses_to_the_database_variables_is_named() {
        let parts = [("LITEGEN_DB_HOST", "h"), ("LITEGEN_DB_USER", "u"), ("LITEGEN_DB_NAME", "n")];
        let c = resolve_connections(&env(&parts), "sqlite://from-yaml.db").unwrap();
        assert!(c.warnings.iter().any(|w| w.contains("litegen.yaml") && w.contains("LITEGEN_DB_HOST")), "{:?}", c.warnings);
        let c = resolve_connections(&env(&parts), DEFAULT).unwrap();
        assert!(c.warnings.is_empty(), "the default isn't a setting: {:?}", c.warnings);
    }
    #[test] fn target_shows_only_a_host_and_a_port_or_a_placeholder() {
        const PLACEHOLDER: &str = "<unparsable Redis URL>";
        // Userinfo with no `@host` must not come out as a host.
        for url in ["redis://:secret", "redis://user:secret", "redis://user:secret@", "redis://:secret@", "redis://", "redis://h:abc",
            "redis://h:", "redis://[secret]:6379", "redis://[::1", "redis://[::1]x", "redis://[::1]:p", "secret", "secret:123"] {
            let t = RedisSettings::from_url(url).target();
            assert_eq!(t, PLACEHOLDER, "{url}");
            assert!(!format!("{:?}", RedisSettings::from_url(url)).contains("secret"), "{url}");
        }
        assert_eq!(RedisSettings::from_url("redis://[::1]:6380").target(), "[::1]:6380");
        assert_eq!(RedisSettings::from_url("rediss://u:p@[2001:db8::7]/0").target(), "[2001:db8::7]:6379");
        assert_eq!(RedisSettings::from_url("redis://cache.internal").target(), "cache.internal:6379");
    }
    #[test] fn a_redis_url_without_a_host_is_refused_and_never_echoed() {
        for url in ["redis://:secret", "redis://user:secret", "redis://u:secret@", "redis://", "redis://h:abc", "redis://[secret]:6379"] {
            let e = resolve_connections(&env(&[("LITEGEN_REDIS_URL", url)]), DEFAULT).unwrap_err();
            assert_eq!(e, "LITEGEN_REDIS_URL is not a valid Redis URL: it needs redis://[user:password@]host[:port]", "{url}");
        }
        assert!(resolve_connections(&env(&[("LITEGEN_REDIS_URL", "rediss://u:p%40ss@h:25061/0")]), DEFAULT).is_ok());
        assert!(resolve_connections(&env(&[("LITEGEN_REDIS_URL", "redis://[::1]:6379")]), DEFAULT).is_ok());
    }
    #[test] fn ipv6_hosts_are_bracketed_and_hosts_with_more_in_them_are_refused() {
        let redis = |host: &str| resolve_connections(&env(&[("LITEGEN_REDIS_HOST", host)]), DEFAULT).map(|c| c.redis.unwrap().url().to_string());
        assert_eq!(redis("::1").unwrap(), "redis://[::1]:6379");
        assert_eq!(redis("[::1]").unwrap(), "redis://[::1]:6379", "already bracketed");
        assert_eq!(redis("2001:db8::7").unwrap(), "redis://[2001:db8::7]:6379");
        let db = |host: &str| resolve_connections(&env(&[("LITEGEN_DB_HOST", host), ("LITEGEN_DB_USER", "u"), ("LITEGEN_DB_NAME", "n")]), DEFAULT).map(|c| c.database_url);
        assert_eq!(db("::1").unwrap(), "postgres://u@[::1]:5432/n");
        assert_eq!(db("[::1]").unwrap(), "postgres://u@[::1]:5432/n");
        for host in ["cache.internal:6379", "cache.internal/0", "user@cache", "cache?x=y", "cache#x", "my host", "cache\tinternal", "[not:ipv6]", "redis://cache", "[host]", "[cache]", "[::1", "::1]", "[]"] {
            let e = redis(host).unwrap_err();
            assert!(e.contains("LITEGEN_REDIS_HOST") && e.contains("LITEGEN_REDIS_PORT"), "{e}");
            assert!(!e.contains("cache") && !e.contains("my host"), "the value must not be echoed: {e}");
            let e = db(host).unwrap_err();
            assert!(e.contains("LITEGEN_DB_HOST") && e.contains("LITEGEN_DATABASE_PORT"), "{e}");
            assert!(!e.contains("cache") && !e.contains("my host"), "the value must not be echoed: {e}");
        }
        // A host that a URL makes pointless is not looked at.
        assert!(resolve_connections(&env(&[("LITEGEN_REDIS_URL", "redis://a:1"), ("LITEGEN_REDIS_HOST", "a:b")]), DEFAULT).is_ok());
    }
    #[test] fn only_non_secret_values_are_trimmed() {
        let c = resolve_connections(&env(&[("LITEGEN_DB_HOST", "h"), ("LITEGEN_DB_USER", " u\n"), ("LITEGEN_DB_NAME", "n\n"),
            ("LITEGEN_DB_PASSWORD", " p w "), ("LITEGEN_DB_SSLMODE", " require\n"), ("LITEGEN_DB_SSLROOTCERT", "/etc/ca.crt\n")]), DEFAULT).unwrap();
        assert_eq!(c.database_url, "postgres://u:%20p%20w%20@h:5432/n?sslmode=require&sslrootcert=%2Fetc%2Fca.crt");
        let r = resolve_connections(&env(&[("LITEGEN_REDIS_HOST", "h"), ("LITEGEN_REDIS_USERNAME", " u\n"), ("LITEGEN_REDIS_PASSWORD", " p w ")]), DEFAULT).unwrap().redis.unwrap();
        assert_eq!(r.url(), "redis://u:%20p%20w%20@h:6379");
    }
    #[test] fn a_breaker_threshold_of_zero_is_refused_but_a_zero_recovery_is_not() {
        let e = resolve_connections(&env(&[("LITEGEN_REDIS_HOST", "h"), ("LITEGEN_REDIS_CIRCUIT_BREAKER_FAILURE_THRESHOLD", "0")]), DEFAULT).unwrap_err();
        assert!(e.contains("LITEGEN_REDIS_CIRCUIT_BREAKER_FAILURE_THRESHOLD") && e.contains("at least 1"), "{e}");
        let r = resolve_connections(&env(&[("LITEGEN_REDIS_HOST", "h"), ("LITEGEN_REDIS_CIRCUIT_BREAKER_RECOVERY_TIMEOUT", "0")]), DEFAULT).unwrap().redis.unwrap();
        assert_eq!(r.breaker_recovery, Duration::ZERO);
    }
    #[test] fn database_user_spellings_that_differ_refuse_to_start() {
        for pair in [["LITEGEN_DATABASE_USER", "LITEGEN_DB_USERNAME"], ["LITEGEN_DATABASE_USER", "LITEGEN_DATABASE_USERNAME"], ["LITEGEN_DB_USER", "LITEGEN_DB_USERNAME"]] {
            let e = resolve_connections(&env(&[(pair[0], "a"), (pair[1], "b"), ("LITEGEN_DB_HOST", "h"), ("LITEGEN_DB_NAME", "n")]), DEFAULT).unwrap_err();
            assert!(e.contains(pair[0]) && e.contains(pair[1]), "{e}");
        }
        let c = resolve_connections(&env(&[("LITEGEN_DATABASE_USER", "a"), ("LITEGEN_DB_USERNAME", "a"), ("LITEGEN_DB_HOST", "h"), ("LITEGEN_DB_NAME", "n")]), DEFAULT).unwrap();
        assert_eq!(c.database_url, "postgres://a@h:5432/n");
    }
    #[test] fn a_database_password_with_hash_colon_and_slash_is_encoded() {
        let c = resolve_connections(&env(&[("LITEGEN_DB_HOST", "h"), ("LITEGEN_DB_USER", "u"), ("LITEGEN_DB_NAME", "n"), ("LITEGEN_DB_PASSWORD", "p#a:s/s?")]), DEFAULT).unwrap();
        assert_eq!(c.database_url, "postgres://u:p%23a%3As%2Fs%3F@h:5432/n");
    }
    #[test] fn kubernetes_service_link_ports_are_ignored_with_a_warning() {
        // A Service named litegen-db, litegen-database or litegen-redis makes Kubernetes set these in every pod.
        let link = "tcp://10.0.0.5:5432";
        let only_the_link_warning = |c: &Connections, name: &str| {
            assert_eq!(c.warnings.len(), 1, "{:?}", c.warnings);
            let w = &c.warnings[0];
            assert!(w.contains(name) && w.contains("Kubernetes service link") && w.contains("ignored") && w.contains("enableServiceLinks: false") && w.contains("Service"), "{w}");
            assert!(!w.contains("10.0.0.5"), "the value must not be echoed: {w}");
        };
        for name in ["LITEGEN_DB_PORT", "LITEGEN_DATABASE_PORT"] {
            // With a host: the port falls back to 5432.
            let c = resolve_connections(&env(&[("LITEGEN_DB_HOST", "h"), ("LITEGEN_DB_USER", "u"), ("LITEGEN_DB_NAME", "n"), (name, link)]), DEFAULT).unwrap();
            assert_eq!(c.database_url, "postgres://u@h:5432/n");
            only_the_link_warning(&c, name);
            // Without a host it is neither a stray part (a refusal) nor an ignored one.
            let c = resolve_connections(&env(&[(name, link)]), "sqlite://from-yaml.db").unwrap();
            assert_eq!(c.database_url, "sqlite://from-yaml.db");
            only_the_link_warning(&c, name);
            // Beside a winning URL it is not named as an ignored part either.
            let c = resolve_connections(&env(&[("LITEGEN_DATABASE_URL", "postgres://x/y"), (name, link)]), DEFAULT).unwrap();
            assert_eq!(c.database_url, "postgres://x/y");
            only_the_link_warning(&c, name);
        }
        // Dropped before the clash check, too.
        let c = resolve_connections(&env(&[("LITEGEN_DATABASE_PORT", link), ("LITEGEN_DB_PORT", "5433"), ("LITEGEN_DB_HOST", "h"), ("LITEGEN_DB_USER", "u"), ("LITEGEN_DB_NAME", "n")]), DEFAULT).unwrap();
        assert_eq!(c.database_url, "postgres://u@h:5433/n");
        only_the_link_warning(&c, "LITEGEN_DATABASE_PORT");

        let link = "tcp://10.0.0.6:6379";
        let c = resolve_connections(&env(&[("LITEGEN_REDIS_HOST", "h"), ("LITEGEN_REDIS_PORT", link)]), DEFAULT).unwrap();
        assert_eq!(c.redis.as_ref().unwrap().url(), "redis://h:6379");
        only_the_link_warning(&c, "LITEGEN_REDIS_PORT");
        let c = resolve_connections(&env(&[("LITEGEN_REDIS_PORT", link)]), DEFAULT).unwrap();
        assert!(c.redis.is_none());
        only_the_link_warning(&c, "LITEGEN_REDIS_PORT");
        let c = resolve_connections(&env(&[("LITEGEN_REDIS_URL", "redis://a:1"), ("LITEGEN_REDIS_PORT", link)]), DEFAULT).unwrap();
        assert_eq!(c.redis.as_ref().unwrap().url(), "redis://a:1");
        only_the_link_warning(&c, "LITEGEN_REDIS_PORT");

        // Each variable is named, and a port that is merely wrong is still refused.
        let c = resolve_connections(&env(&[("LITEGEN_DB_PORT", link), ("LITEGEN_REDIS_PORT", link)]), DEFAULT).unwrap();
        assert_eq!(c.warnings.len(), 2, "{:?}", c.warnings);
        assert!(resolve_connections(&env(&[("LITEGEN_REDIS_HOST", "h"), ("LITEGEN_REDIS_PORT", "tcp:/10.0.0.6:6379")]), DEFAULT).is_err());
    }
    #[test] fn db_parts_without_a_host_refuse_to_start() {
        // A user and a name with no host would otherwise fall back to SQLite without a word.
        let e = resolve_connections(&env(&[("LITEGEN_DB_USER", "u"), ("LITEGEN_DATABASE_NAME", "n"), ("LITEGEN_DB_PASSWORD", "hunter2")]), DEFAULT).unwrap_err();
        assert!(e.contains("LITEGEN_DATABASE_HOST"), "{e}");
        assert!(e.contains("LITEGEN_DB_USER") && e.contains("LITEGEN_DATABASE_NAME") && e.contains("LITEGEN_DB_PASSWORD"), "{e}");
        assert!(!e.contains("hunter2"), "{e}");
        // One stray part is enough, in either spelling.
        for name in ["LITEGEN_DATABASE_PORT", "LITEGEN_DB_SSLMODE", "LITEGEN_DATABASE_SSLROOTCERT", "LITEGEN_DB_USERNAME"] {
            let e = resolve_connections(&env(&[(name, "x")]), "sqlite://from-yaml.db").unwrap_err();
            assert!(e.contains("LITEGEN_DATABASE_HOST") && e.contains(name), "{e}");
        }
        // A URL that wins settles it: no error, and the host-less parts change nothing.
        for url in ["LITEGEN_DATABASE_URL", "LITEGEN_DB_URL", "LITEGEN__DATABASE_URL"] {
            let c = resolve_connections(&env(&[(url, "postgres://x/y"), ("LITEGEN_DB_USER", "u")]), "postgres://x/y").unwrap();
            assert_eq!(c.database_url, "postgres://x/y");
        }
    }
    #[test] fn a_lone_db_part_beside_a_url_is_named_as_ignored() {
        for (url, configured) in [("LITEGEN_DATABASE_URL", DEFAULT), ("LITEGEN_DB_URL", DEFAULT), ("LITEGEN__DATABASE_URL", "postgres://x/y")] {
            let c = resolve_connections(&env(&[(url, "postgres://x/y"), ("LITEGEN_DB_USER", "u")]), configured).unwrap();
            assert_eq!(c.database_url, "postgres://x/y");
            assert!(c.warnings.iter().any(|w| w.contains("LITEGEN_DB_USER") && w.contains(url)), "{url}: {:?}", c.warnings);
        }
    }
    #[test] fn redis_parts_without_a_host_warn_and_leave_redis_off() {
        let c = resolve_connections(&env(&[("LITEGEN_REDIS_PORT", "6380"), ("LITEGEN_REDIS_PASSWORD", "hunter2"), ("LITEGEN_REDIS_SSL", "true"),
            ("LITEGEN_REDIS_USERNAME", "u"), ("LITEGEN_REDIS_SSL_CA_CERTS", "/etc/ca.pem"), ("LITEGEN_REDIS_SSL_CERT_REQS", "none")]), DEFAULT).unwrap();
        assert!(c.redis.is_none());
        let w = c.warnings.iter().find(|w| w.contains("LITEGEN_REDIS_PORT")).unwrap_or_else(|| panic!("{:?}", c.warnings));
        for name in ["LITEGEN_REDIS_PASSWORD", "LITEGEN_REDIS_USERNAME", "LITEGEN_REDIS_SSL", "LITEGEN_REDIS_SSL_CA_CERTS", "LITEGEN_REDIS_SSL_CERT_REQS", "LITEGEN_REDIS_URL", "LITEGEN_REDIS_HOST"] {
            assert!(w.contains(name), "{name} missing from {w}");
        }
        assert!(w.contains("off"), "{w}");
        assert!(!w.contains("hunter2"), "{w}");
        // The outage settings alone say nothing: they have no server to talk about.
        let c = resolve_connections(&env(&[("LITEGEN_FAIL_CLOSED_RATE_LIMIT_ENFORCEMENT", "true")]), DEFAULT).unwrap();
        assert!(c.redis.is_none() && c.warnings.is_empty(), "{:?}", c.warnings);
    }
    // ── The default database (S2) ──

    #[test] fn the_default_counts_as_unset_only_when_nothing_names_a_database() {
        assert!(resolve_connections(&env(&[]), DEFAULT).unwrap().database_is_default, "nothing names a database");
        let named = |pairs: &[(&str, &str)], configured: &str| resolve_connections(&env(pairs), configured).unwrap().database_is_default;
        assert!(!named(&[("LITEGEN_DATABASE_URL", "sqlite://x.db")], DEFAULT), "LITEGEN_DATABASE_URL");
        assert!(!named(&[("LITEGEN_DB_URL", "sqlite://x.db")], DEFAULT), "LITEGEN_DB_URL");
        assert!(!named(&[("LITEGEN__DATABASE_URL", DEFAULT)], DEFAULT), "LITEGEN__DATABASE_URL, even set to the default's own text");
        assert!(!named(&[("LITEGEN_DB_HOST", "h"), ("LITEGEN_DB_USER", "u"), ("LITEGEN_DB_NAME", "n")], DEFAULT), "the parts");
        assert!(!named(&[], "sqlite://from-yaml.db"), "a URL from the config file");
    }
    #[test] fn a_litegen_db_from_the_old_default_is_used_until_data_litegen_db_exists() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(default_database_in(dir.path()), (DEFAULT.to_string(), None), "an empty working directory");
        // Both files named: the old one and data/litegen.db.
        let names_both = |w: &str| w.contains("data/litegen.db") && w.matches("litegen.db").count() >= 2;

        std::fs::write(dir.path().join("litegen.db"), "").unwrap();
        let (url, warning) = default_database_in(dir.path());
        let warning = warning.unwrap_or_else(|| panic!("litegen.db alone: no warning, and {url} is opened"));
        assert_eq!(url, "sqlite://litegen.db", "litegen.db, which the old default opened, is still used");
        assert!(names_both(&warning) && warning.contains("LITEGEN_DATABASE_URL"), "{warning}");

        std::fs::create_dir(dir.path().join("data")).unwrap();
        std::fs::write(dir.path().join("data/litegen.db"), "").unwrap();
        let (url, warning) = default_database_in(dir.path());
        let warning = warning.unwrap_or_else(|| panic!("both files: no warning, and {url} is opened"));
        assert_eq!(url, DEFAULT, "data/litegen.db exists, so it is the database");
        assert!(names_both(&warning) && warning.contains("not used"), "{warning}");
    }
    #[test] fn warnings_never_hold_a_value() {
        let c = resolve_connections(&env(&[("LITEGEN_REDIS_URL", "redis://a:6379"), ("LITEGEN_REDIS_HOST", "secret-host"),
            ("LITEGEN_REDIS_PASSWORD", "hunter2"), ("LITEGEN_DATABASE_URL", "postgres://x/y"), ("LITEGEN_DB_HOST", "other-host"),
            ("LITEGEN_DB_PASSWORD", "hunter3")]), DEFAULT).unwrap();
        assert!(!c.warnings.is_empty());
        for w in &c.warnings {
            assert!(!["secret-host", "hunter2", "other-host", "hunter3"].iter().any(|v| w.contains(v)), "{w}");
        }
    }
}
