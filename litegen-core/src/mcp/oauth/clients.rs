//! Who an OAuth client is. A `client_id` that is an `https` URL with a path
//! is a Client ID Metadata Document (CIMD): LiteGen fetches the JSON it
//! names, checks it, and keeps it for a day; Claude and ChatGPT identify
//! themselves this way. Any other id was minted by dynamic registration
//! (RFC 7591, `POST /oauth/register`). PKCE protects every code exchange; no
//! client has a secret. A registered client is public, and so is a document
//! unless it declares `private_key_jwt` and publishes its keys, as ChatGPT's
//! does: then it signs every token request (see [`super::assertion`]).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};

use super::crypto::random_token;
use super::store::{McpOAuthStore, OAuthClient, Registration};
use super::OAuthShared;
use crate::util::ssrf::{self, CappedDownloadError};

/// How long a fetched metadata document is trusted before it is fetched again.
const METADATA_TTL: chrono::Duration = chrono::Duration::hours(24);
const METADATA_TIMEOUT: Duration = Duration::from_secs(5);
const METADATA_MAX_BYTES: u64 = 64 * 1024;
/// The largest registration request.
pub const REGISTER_MAX_BYTES: usize = 16 * 1024;
const MAX_REDIRECT_URIS: usize = 10;
const MAX_NAME_CHARS: usize = 200;

/// Fetches a client's metadata document.
#[async_trait]
pub trait MetadataFetcher: Send + Sync {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, String>;
}

/// Fetches metadata documents from the public internet only: `https`, no
/// redirects, 5 s, at most 64 KiB, and from public addresses only, the host
/// pinned to the addresses that were checked (anyone can name a URL here).
pub struct PublicFetcher;

#[async_trait]
impl MetadataFetcher for PublicFetcher {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, String> {
        if !url.starts_with("https://") {
            return Err("a client metadata document must be served over https".into());
        }
        let client = ssrf::pinned_public_client(url, METADATA_TIMEOUT).await.map_err(|e| e.to_string())?;
        match ssrf::download_capped(&client, url, METADATA_MAX_BYTES).await {
            Ok((bytes, _)) => Ok(bytes),
            Err(CappedDownloadError::Status(status)) => Err(format!("the document answered {status}")),
            Err(CappedDownloadError::TooLarge { max, .. }) => Err(format!("the document is over {max} bytes")),
            Err(CappedDownloadError::Transport(e)) => Err(e),
        }
    }
}

/// Why a client could not be identified.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientError {
    /// LiteGen has no client with this id.
    Unknown(String),
    /// The client's metadata document could not be fetched or breaks a rule.
    InvalidMetadata(String),
    Store(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Unknown(m) | ClientError::InvalidMetadata(m) | ClientError::Store(m) => f.write_str(m),
        }
    }
}

impl From<sqlx::Error> for ClientError {
    fn from(e: sqlx::Error) -> Self {
        ClientError::Store(e.to_string())
    }
}

/// The client `client_id` names: its metadata document (fetched when not
/// cached or older than a day), or its dynamic registration.
pub async fn resolve_client(
    store: &dyn McpOAuthStore,
    fetcher: &dyn MetadataFetcher,
    client_id: &str,
    now: DateTime<Utc>,
) -> Result<OAuthClient, ClientError> {
    let known = store.get_client(client_id).await?;
    if !is_metadata_url(client_id) {
        return known
            .filter(|c| c.registration == Registration::Dcr)
            .ok_or_else(|| ClientError::Unknown(format!("LiteGen has no client with the id '{client_id}'")));
    }
    if let Some(fresh) = known.as_ref().filter(|c| c.registration == Registration::Cimd && now - c.refreshed_at < METADATA_TTL) {
        return Ok(fresh.clone());
    }
    let bytes = fetcher
        .fetch(client_id)
        .await
        .map_err(|e| ClientError::InvalidMetadata(format!("the client's metadata document could not be fetched: {e}")))?;
    let client = metadata_client(client_id, &bytes, now)?;
    // A fetch again keeps when the client registered and was last used.
    let client = match known {
        Some(k) => OAuthClient { created_at: k.created_at, last_used_at: k.last_used_at, ..client },
        None => client,
    };
    store.upsert_client(&client).await?;
    Ok(client)
}

/// A metadata document's URL: `https`, with a path, in canonical form, with
/// no credentials and no fragment.
fn is_metadata_url(client_id: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(client_id) else { return false };
    url.scheme() == "https"
        && url.host_str().is_some()
        && url.path() != "/"
        && url.fragment().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.as_str() == client_id
}

/// The client a metadata document fetched at `now` describes, or what is
/// wrong with it.
fn metadata_client(client_id: &str, bytes: &[u8], now: DateTime<Utc>) -> Result<OAuthClient, ClientError> {
    let invalid = |m: &str| ClientError::InvalidMetadata(format!("the client's metadata document {m}"));
    let doc = match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(doc)) => doc,
        _ => return Err(invalid("is not a JSON object")),
    };
    if doc.get("client_id").and_then(Value::as_str) != Some(client_id) {
        return Err(invalid("names another client_id than its own URL"));
    }
    let redirect_uris = redirect_uris(&doc).map_err(|e| invalid(&format!("is invalid: {e}")))?;
    match doc.get("token_endpoint_auth_method").and_then(Value::as_str) {
        None | Some("none") => {}
        // A confidential client (CIMD §6.2): it signs with keys it publishes.
        Some("private_key_jwt") => {
            let https_keys = doc.get("jwks_uri").and_then(Value::as_str).is_some_and(|uri| reqwest::Url::parse(uri).is_ok_and(|u| u.scheme() == "https"));
            if !https_keys && !doc.get("jwks").is_some_and(Value::is_object) {
                return Err(invalid("asks for private_key_jwt but publishes no keys: it needs an https jwks_uri or a jwks"));
            }
        }
        Some(other) => return Err(invalid(&format!("asks for token_endpoint_auth_method '{other}'; LiteGen supports none and private_key_jwt"))),
    }
    if doc.contains_key("client_secret") {
        return Err(invalid("carries a client_secret, which a public document must not"));
    }
    if let Some(types) = doc.get("grant_types") {
        let listed = types.as_array().is_some_and(|types| types.iter().any(|t| t.as_str() == Some("authorization_code")));
        if !listed {
            return Err(invalid("lists grant_types without authorization_code, the only way to sign in to LiteGen"));
        }
    }
    let host = reqwest::Url::parse(client_id).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default();
    Ok(OAuthClient {
        client_id: client_id.to_string(),
        registration: Registration::Cimd,
        client_name: client_name(&doc).unwrap_or(host),
        redirect_uris,
        logo_uri: https_uri(&doc, "logo_uri"),
        client_uri: https_uri(&doc, "client_uri"),
        metadata: Value::Object(doc),
        created_at: now,
        refreshed_at: now,
        last_used_at: None,
    })
}

/// Whether `requested` is one of a client's `registered` redirect URIs: the
/// same string, or, for `http` on a loopback host, the same URI on another
/// port (RFC 8252 §7.3: a native app picks its port when it runs).
pub fn redirect_allowed(registered: &[String], requested: &str) -> bool {
    if registered.iter().any(|r| r == requested) {
        return true;
    }
    let Ok(req) = reqwest::Url::parse(requested) else { return false };
    if req.scheme() != "http" || !is_loopback_host(req.host_str()) || req.fragment().is_some() {
        return false;
    }
    registered.iter().filter_map(|r| reqwest::Url::parse(r).ok()).any(|reg| {
        reg.scheme() == "http"
            && reg.host_str() == req.host_str()
            && reg.path() == req.path()
            && reg.query() == req.query()
            && reg.username() == req.username()
            && reg.password() == req.password()
    })
}

pub(super) fn is_loopback_host(host: Option<&str>) -> bool {
    matches!(host, Some("localhost" | "127.0.0.1" | "[::1]"))
}

/// A redirect URI LiteGen will send a code to: absolute `https`, or `http` on
/// a loopback host, with no credentials and no fragment.
fn valid_redirect_uri(uri: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(uri) else { return false };
    let scheme_ok = match url.scheme() {
        "https" => url.host_str().is_some(),
        "http" => is_loopback_host(url.host_str()),
        _ => false,
    };
    scheme_ok && url.fragment().is_none() && url.username().is_empty() && url.password().is_none()
}

/// Metadata's `redirect_uris`: one to ten valid URIs, or why not.
fn redirect_uris(meta: &Map<String, Value>) -> Result<Vec<String>, String> {
    let list = meta
        .get("redirect_uris")
        .and_then(Value::as_array)
        .filter(|l| !l.is_empty())
        .ok_or("redirect_uris must list at least one URI")?;
    if list.len() > MAX_REDIRECT_URIS {
        return Err(format!("redirect_uris may list at most {MAX_REDIRECT_URIS} URIs"));
    }
    list.iter()
        .map(|v| match v.as_str() {
            Some(uri) if valid_redirect_uri(uri) => Ok(uri.to_string()),
            Some(uri) => Err(format!("redirect URI '{uri}' must be https, or http on localhost, 127.0.0.1 or [::1], without a fragment")),
            None => Err("redirect_uris must be strings".to_string()),
        })
        .collect()
}

fn client_name(meta: &Map<String, Value>) -> Option<String> {
    let name = meta.get("client_name")?.as_str()?.trim();
    (!name.is_empty()).then(|| name.chars().take(MAX_NAME_CHARS).collect())
}

/// A metadata URI shown to people (a logo, a home page): kept only if `https`.
fn https_uri(meta: &Map<String, Value>, key: &str) -> Option<String> {
    let uri = meta.get(key)?.as_str()?;
    reqwest::Url::parse(uri).ok().filter(|u| u.scheme() == "https").map(|_| uri.to_string())
}

/// The values of `key`'s list that LiteGen supports, in order, once each
/// (all of `supported` when absent); `required` must be among them. RFC 7591
/// lets a server register other values than a client asked for, and its
/// answer says which.
fn supported_list(meta: &Map<String, Value>, key: &str, supported: &[&str], required: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    match meta.get(key) {
        None => out.extend(supported.iter().map(|s| s.to_string())),
        Some(Value::Array(list)) => {
            for value in list.iter().filter_map(Value::as_str).filter(|v| supported.contains(v)) {
                if !out.iter().any(|o| o == value) {
                    out.push(value.to_string());
                }
            }
        }
        Some(_) => return Err(format!("{key} must be a list")),
    }
    if !out.iter().any(|v| v == required) {
        return Err(format!("{key} must include {required}: LiteGen supports {}", supported.join(", ")));
    }
    Ok(out)
}

/// Whether a client takes refresh tokens: its registration or document
/// lists the refresh_token grant, or lists no grant types at all.
pub fn uses_refresh_tokens(client: &OAuthClient) -> bool {
    match client.metadata.get("grant_types").and_then(Value::as_array) {
        Some(types) => types.iter().any(|t| t.as_str() == Some("refresh_token")),
        None => true,
    }
}

/// `POST /oauth/register` (RFC 7591): a public client declares its redirect
/// URIs and gets a `client_id`. LiteGen registers what it supports of what
/// the client asks for: no client secret, the authorization-code flow and
/// (unless the client leaves it out) refresh tokens.
pub async fn register(State(shared): State<Arc<OAuthShared>>, body: Bytes) -> Response {
    let Some(store) = shared.state.db.mcp_oauth() else {
        return registration_error(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable", "OAuth is not available");
    };
    let mut meta = match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(meta)) => meta,
        _ => return registration_error(StatusCode::BAD_REQUEST, "invalid_client_metadata", "the body must be a JSON object"),
    };
    let redirect_uris = match redirect_uris(&meta) {
        Ok(uris) => uris,
        Err(e) => return registration_error(StatusCode::BAD_REQUEST, "invalid_redirect_uri", &e),
    };
    if meta.get("token_endpoint_auth_method").is_some_and(|m| m.as_str() != Some("none")) {
        return registration_error(
            StatusCode::BAD_REQUEST,
            "invalid_client_metadata",
            "token_endpoint_auth_method must be \"none\": LiteGen's clients are public and use PKCE",
        );
    }
    let grant_types = supported_list(&meta, "grant_types", &["authorization_code", "refresh_token"], "authorization_code");
    let response_types = supported_list(&meta, "response_types", &["code"], "code");
    let (grant_types, response_types) = match (grant_types, response_types) {
        (Ok(g), Ok(r)) => (g, r),
        (Err(e), _) | (_, Err(e)) => return registration_error(StatusCode::BAD_REQUEST, "invalid_client_metadata", &e),
    };
    // Kept as registered: the token endpoint reads the grant types from it.
    meta.insert("grant_types".into(), json!(grant_types));
    meta.insert("response_types".into(), json!(response_types));

    let now = Utc::now();
    let name = client_name(&meta);
    let client = OAuthClient {
        client_id: random_token("lgc_", 16),
        registration: Registration::Dcr,
        client_name: name.clone().unwrap_or_else(|| "Unnamed client".to_string()),
        redirect_uris,
        logo_uri: https_uri(&meta, "logo_uri"),
        client_uri: https_uri(&meta, "client_uri"),
        metadata: Value::Object(meta),
        created_at: now,
        refreshed_at: now,
        last_used_at: None,
    };
    if let Err(e) = store.upsert_client(&client).await {
        tracing::error!(error = %e, "OAuth client registration failed");
        return registration_error(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "the client could not be stored");
    }

    let mut answer = json!({
        "client_id": client.client_id,
        "client_id_issued_at": now.timestamp(),
        "redirect_uris": client.redirect_uris,
        "token_endpoint_auth_method": "none",
        "grant_types": grant_types,
        "response_types": response_types,
    });
    if let Some(name) = name {
        answer["client_name"] = Value::String(name);
    }
    for (key, value) in [("client_uri", client.client_uri), ("logo_uri", client.logo_uri)] {
        if let Some(value) = value {
            answer[key] = Value::String(value);
        }
    }
    (StatusCode::CREATED, [(header::CACHE_CONTROL, "no-store")], Json(answer)).into_response()
}

/// An RFC 7591 error answer.
fn registration_error(status: StatusCode, error: &str, description: &str) -> Response {
    (status, [(header::CACHE_CONTROL, "no-store")], Json(json!({ "error": error, "error_description": description }))).into_response()
}
