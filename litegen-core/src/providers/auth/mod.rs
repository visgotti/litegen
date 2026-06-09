//! Flexible per-provider authentication.
//!
//! Auth scheme is intrinsic to a vendor (a constant declared by each provider);
//! the user supplies only *credentials*. This module models the schemes litegen
//! needs across all integrated vendors and applies them to outbound requests:
//!
//! - `Header` / `QueryParam` — API key in a header (Bearer, "Token ", x-key,
//!   x-goog-api-key, Api-Key, API-KEY) or a query parameter (Google `?key=`).
//! - `AwsSigV4` — AWS Signature V4 request signing (Amazon Bedrock).
//! - `KlingJwt` — Kling per-request HS256 JWT minted from access/secret key.
//! - `TencentTc3` — Tencent Cloud TC3-HMAC-SHA256 request signing (Hunyuan).
//!
//! Simple schemes (`Header`, `QueryParam`, `KlingJwt`) are applied to a
//! `reqwest::RequestBuilder` via [`apply`]. Signing schemes (`AwsSigV4`,
//! `TencentTc3`) need the finalized request body to compute the signature, so
//! their providers call [`sigv4::sign`] / [`tc3::sign`] directly with the body
//! bytes and add the returned headers.

use crate::providers::ProviderError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod kling_jwt;
pub mod sigv4;
pub mod tc3;

/// How a provider authenticates outbound HTTP requests.
///
/// Declared per-provider in code (NOT user-configurable). Serializable so the
/// `GET /v1/providers/schema` endpoint can publish each provider's scheme for
/// the dashboard to render credential forms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "scheme", rename_all = "snake_case")]
pub enum AuthSpec {
    /// API key placed in a request header. `value_prefix` is prepended to the
    /// key (e.g. `"Bearer "`, `"Token "`, or `""` for raw custom headers).
    Header { name: String, value_prefix: String },

    /// API key placed in a query parameter (e.g. Google `?key=`).
    QueryParam { name: String },

    /// AWS Signature V4 request signing. `service` is the SigV4 service name
    /// (e.g. `"bedrock"`); `default_region` is used when no per-request region
    /// is supplied.
    AwsSigV4 { service: String, default_region: String },

    /// Kling per-request JWT: HS256 over `{iss: access_key, exp: now+1800,
    /// nbf: now-5}`, sent as `Authorization: Bearer <jwt>`.
    KlingJwt,

    /// Tencent Cloud TC3-HMAC-SHA256 request signing. `service` is the API
    /// service name (e.g. `"hunyuan"`, `"vclm"`).
    TencentTc3 { service: String, default_region: String },
}

impl AuthSpec {
    /// Convenience constructor for the common `Authorization: Bearer <key>`.
    pub fn bearer() -> Self {
        AuthSpec::Header {
            name: "Authorization".to_string(),
            value_prefix: "Bearer ".to_string(),
        }
    }

    /// Convenience constructor for a raw custom header (`<name>: <key>`).
    pub fn raw_header(name: &str) -> Self {
        AuthSpec::Header {
            name: name.to_string(),
            value_prefix: String::new(),
        }
    }

    /// Stable identifier for the scheme (used in the providers schema endpoint).
    pub fn scheme_name(&self) -> &'static str {
        match self {
            AuthSpec::Header { .. } => "header",
            AuthSpec::QueryParam { .. } => "query_param",
            AuthSpec::AwsSigV4 { .. } => "aws_sigv4",
            AuthSpec::KlingJwt => "kling_jwt",
            AuthSpec::TencentTc3 { .. } => "tencent_tc3",
        }
    }

    /// Credential field names a user must supply for this scheme. Drives the
    /// registration skip-guard and the providers schema endpoint.
    pub fn required_fields(&self) -> &'static [&'static str] {
        match self {
            AuthSpec::Header { .. } | AuthSpec::QueryParam { .. } => &["api_key"],
            AuthSpec::AwsSigV4 { .. } => &["key_id", "key_secret", "region"],
            AuthSpec::KlingJwt => &["key_id", "key_secret"],
            AuthSpec::TencentTc3 { .. } => &["key_id", "key_secret", "region"],
        }
    }

    /// Whether the supplied credentials satisfy this scheme's requirements.
    pub fn is_satisfied_by(&self, c: &ProviderCredentials) -> bool {
        self.required_fields().iter().all(|f| match *f {
            // A non-empty pool satisfies the scheme on its own: each pooled
            // entry carries its own credential (and region, or a default).
            "api_key" => c.api_key.as_deref().is_some_and(|k| !k.is_empty()) || !c.api_keys.is_empty(),
            "key_id" => c.key_id.as_deref().is_some_and(|k| !k.is_empty()) || !c.credential_sets.is_empty(),
            "key_secret" => c.key_secret.as_deref().is_some_and(|k| !k.is_empty()) || !c.credential_sets.is_empty(),
            "region" => c.region.as_deref().is_some_and(|k| !k.is_empty()) || !c.credential_sets.is_empty(),
            _ => false,
        })
    }
}

/// Credentials supplied by the user. The union across all schemes; each scheme
/// reads only the fields it needs. Plain data — providers build their own
/// weighted [`crate::providers::ApiKeyPool`] from `api_keys` if present.
#[derive(Debug, Clone, Default)]
pub struct ProviderCredentials {
    /// Single API key (header/query/bearer/jwt-secret-less schemes).
    pub api_key: Option<String>,
    /// Weighted multi-key pool entries (header schemes).
    pub api_keys: Vec<crate::types::ApiKeyEntry>,
    /// Weighted multi-credential pool entries (signing schemes: SigV4/TC3/JWT).
    pub credential_sets: Vec<crate::types::CredentialEntry>,
    /// Access key id (SigV4) / secret id (TC3) / access key (Kling).
    pub key_id: Option<String>,
    /// Secret access key (SigV4) / secret key (TC3, Kling).
    pub key_secret: Option<String>,
    /// Region (SigV4 / TC3). Non-secret; part of the host and credential scope.
    pub region: Option<String>,
    /// Reserved for future auxiliary fields (e.g. group_id).
    pub extra: HashMap<String, String>,
}

impl ProviderCredentials {
    /// Whether *any* credential is present (cheap pre-check for the skip-guard).
    pub fn any_present(&self) -> bool {
        self.api_key.as_deref().is_some_and(|k| !k.is_empty())
            || !self.api_keys.is_empty()
            || !self.credential_sets.is_empty()
            || self.key_id.as_deref().is_some_and(|k| !k.is_empty())
    }

    /// Return a clone with `api_key` overridden — used by pooled providers that
    /// pick a key per request before applying a header scheme.
    pub fn with_api_key(&self, key: String) -> Self {
        let mut c = self.clone();
        c.api_key = Some(key);
        c
    }

    /// Return a clone with the signing credential (`key_id`/`key_secret`, and
    /// `region` when the entry carries one) overridden — used by signing
    /// providers that pick a credential set per request from a [`CredentialPool`]
    /// before signing. An entry without a region keeps the existing/default one.
    pub fn with_signing(&self, entry: &crate::types::CredentialEntry) -> Self {
        let mut c = self.clone();
        c.key_id = Some(entry.key_id.clone());
        c.key_secret = Some(entry.key_secret.clone());
        if entry.region.is_some() {
            c.region = entry.region.clone();
        }
        c
    }

    /// Build from a decrypted stored credential JSON, e.g. `{"api_key":"sk-…"}`
    /// or `{"key_id":"…","key_secret":"…","region":"…"}`. Recognized keys map to
    /// their fields; any other string-valued key lands in `extra` (non-string
    /// values are ignored).
    ///
    /// `api_keys` enables weighted multi-key BYO load balancing for bearer
    /// schemes. It accepts either a structured array
    /// (`[{"key":"sk-1","weight":3,"label":"x"}, …]`, missing `weight` defaults
    /// to 1) or the env-style string form (`"sk-1:3,sk-2"`). Entries with an
    /// empty key are dropped, since an empty bearer token is never valid. A
    /// coexisting single `api_key` is preserved as a fallback.
    ///
    /// `credential_sets` is the signing-scheme analogue (SigV4/TC3/JWT): an
    /// array of `{"key_id","key_secret","region"?,"weight"?,"label"?}` objects
    /// (missing `weight` defaults to 1, `region` optional). Entries missing
    /// `key_id` or `key_secret` are dropped.
    pub fn from_json(v: &serde_json::Value) -> Self {
        let obj = v.as_object();
        let s = |k: &str| {
            obj.and_then(|o| o.get(k))
                .and_then(|x| x.as_str())
                .map(|x| x.to_string())
        };
        let api_keys = match obj.and_then(|o| o.get("api_keys")) {
            Some(serde_json::Value::Array(_)) => {
                serde_json::from_value::<Vec<crate::types::ApiKeyEntry>>(
                    obj.and_then(|o| o.get("api_keys")).cloned().unwrap_or_default(),
                )
                .unwrap_or_default()
            }
            Some(serde_json::Value::String(raw)) => crate::providers::parse_api_keys(raw),
            _ => Vec::new(),
        }
        .into_iter()
        .filter(|e| !e.key.is_empty())
        .collect();
        // Signing pool: array of {key_id, key_secret, region?, weight?, label?}.
        // Entries missing either secret field are dropped (can't sign with them).
        let credential_sets: Vec<crate::types::CredentialEntry> = obj
            .and_then(|o| o.get("credential_sets"))
            .filter(|v| v.is_array())
            .and_then(|v| serde_json::from_value::<Vec<crate::types::CredentialEntry>>(v.clone()).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|e| !e.key_id.is_empty() && !e.key_secret.is_empty())
            .collect();
        let mut extra = HashMap::new();
        if let Some(o) = obj {
            for (k, val) in o {
                if !["api_key", "key_id", "key_secret", "region", "api_keys", "credential_sets"]
                    .contains(&k.as_str())
                {
                    if let Some(sv) = val.as_str() {
                        extra.insert(k.clone(), sv.to_string());
                    }
                }
            }
        }
        ProviderCredentials {
            api_key: s("api_key"),
            api_keys,
            credential_sets,
            key_id: s("key_id"),
            key_secret: s("key_secret"),
            region: s("region"),
            extra,
        }
    }
}

/// Apply a *non-signing* auth scheme to a request builder.
///
/// Handles `Header`, `QueryParam`, and `KlingJwt`. Signing schemes
/// (`AwsSigV4`, `TencentTc3`) return an error here — their providers must call
/// [`sigv4::sign`] / [`tc3::sign`] with the request body bytes instead.
pub fn apply(
    spec: &AuthSpec,
    creds: &ProviderCredentials,
    builder: reqwest::RequestBuilder,
) -> Result<reqwest::RequestBuilder, ProviderError> {
    match spec {
        AuthSpec::Header { name, value_prefix } => {
            let key = require_api_key(creds)?;
            Ok(builder.header(name.as_str(), format!("{value_prefix}{key}")))
        }
        AuthSpec::QueryParam { name } => {
            let key = require_api_key(creds)?;
            Ok(builder.query(&[(name.as_str(), key.as_str())]))
        }
        AuthSpec::KlingJwt => {
            let jwt = kling_jwt::mint(creds)?;
            Ok(builder.header("Authorization", format!("Bearer {jwt}")))
        }
        AuthSpec::AwsSigV4 { .. } | AuthSpec::TencentTc3 { .. } => {
            Err(ProviderError::InvalidRequest(format!(
                "auth scheme '{}' must be applied via its signer with request body bytes",
                spec.scheme_name()
            )))
        }
    }
}

fn require_api_key(creds: &ProviderCredentials) -> Result<String, ProviderError> {
    creds
        .api_key
        .clone()
        .filter(|k| !k.is_empty())
        .ok_or_else(|| ProviderError::NotConfigured("missing api_key".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds_key(k: &str) -> ProviderCredentials {
        ProviderCredentials { api_key: Some(k.to_string()), ..Default::default() }
    }

    #[test]
    fn required_fields_per_scheme() {
        assert_eq!(AuthSpec::bearer().required_fields(), &["api_key"]);
        assert_eq!(AuthSpec::raw_header("x-key").required_fields(), &["api_key"]);
        assert_eq!(
            AuthSpec::QueryParam { name: "key".into() }.required_fields(),
            &["api_key"]
        );
        assert_eq!(
            AuthSpec::AwsSigV4 { service: "bedrock".into(), default_region: "us-east-1".into() }
                .required_fields(),
            &["key_id", "key_secret", "region"]
        );
        assert_eq!(AuthSpec::KlingJwt.required_fields(), &["key_id", "key_secret"]);
        assert_eq!(
            AuthSpec::TencentTc3 { service: "hunyuan".into(), default_region: "ap-guangzhou".into() }
                .required_fields(),
            &["key_id", "key_secret", "region"]
        );
    }

    #[test]
    fn is_satisfied_by_checks_required_fields() {
        let bearer = AuthSpec::bearer();
        assert!(bearer.is_satisfied_by(&creds_key("sk-1")));
        assert!(!bearer.is_satisfied_by(&ProviderCredentials::default()));

        let sigv4 = AuthSpec::AwsSigV4 { service: "bedrock".into(), default_region: "us-east-1".into() };
        let mut c = ProviderCredentials {
            key_id: Some("AKIA".into()),
            key_secret: Some("secret".into()),
            ..Default::default()
        };
        assert!(!sigv4.is_satisfied_by(&c), "missing region");
        c.region = Some("us-east-1".into());
        assert!(sigv4.is_satisfied_by(&c));
    }

    #[test]
    fn apply_header_bearer() {
        let client = reqwest::Client::new();
        let b = apply(&AuthSpec::bearer(), &creds_key("sk-abc"), client.get("http://x")).unwrap();
        let req = b.build().unwrap();
        assert_eq!(req.headers().get("authorization").unwrap(), "Bearer sk-abc");
    }

    #[test]
    fn apply_raw_header() {
        let client = reqwest::Client::new();
        let b = apply(&AuthSpec::raw_header("x-key"), &creds_key("bfl-1"), client.get("http://x")).unwrap();
        let req = b.build().unwrap();
        assert_eq!(req.headers().get("x-key").unwrap(), "bfl-1");
    }

    #[test]
    fn apply_query_param() {
        let client = reqwest::Client::new();
        let b = apply(
            &AuthSpec::QueryParam { name: "key".into() },
            &creds_key("g-1"),
            client.get("http://x/path"),
        )
        .unwrap();
        let req = b.build().unwrap();
        assert_eq!(req.url().query(), Some("key=g-1"));
    }

    #[test]
    fn apply_signing_schemes_error_without_body() {
        let client = reqwest::Client::new();
        let spec = AuthSpec::AwsSigV4 { service: "bedrock".into(), default_region: "us-east-1".into() };
        let creds = ProviderCredentials {
            key_id: Some("AKIA".into()),
            key_secret: Some("s".into()),
            region: Some("us-east-1".into()),
            ..Default::default()
        };
        assert!(apply(&spec, &creds, client.get("http://x")).is_err());
    }

    #[test]
    fn missing_api_key_errors() {
        let client = reqwest::Client::new();
        assert!(apply(&AuthSpec::bearer(), &ProviderCredentials::default(), client.get("http://x")).is_err());
    }

    #[test]
    fn provider_credentials_from_json() {
        use serde_json::json;

        // Bearer-style: single api_key.
        let c = ProviderCredentials::from_json(&json!({"api_key": "sk-x"}));
        assert_eq!(c.api_key.as_deref(), Some("sk-x"));
        assert!(c.key_id.is_none());

        // Signing-style: key_id / key_secret / region.
        let c = ProviderCredentials::from_json(
            &json!({"key_id": "a", "key_secret": "b", "region": "us"}),
        );
        assert_eq!(c.key_id.as_deref(), Some("a"));
        assert_eq!(c.key_secret.as_deref(), Some("b"));
        assert_eq!(c.region.as_deref(), Some("us"));
        assert!(c.api_key.is_none());

        // Unknown string key → extra; non-string values ignored.
        let c = ProviderCredentials::from_json(
            &json!({"api_key": "sk-y", "group_id": "g-1", "ignored_num": 5}),
        );
        assert_eq!(c.api_key.as_deref(), Some("sk-y"));
        assert_eq!(c.extra.get("group_id").map(String::as_str), Some("g-1"));
        assert!(!c.extra.contains_key("ignored_num"));

        // No api_keys present → empty pool entries.
        let c = ProviderCredentials::from_json(&json!({"api_key": "sk-z"}));
        assert!(c.api_keys.is_empty());
    }

    #[test]
    fn from_json_parses_api_keys_array() {
        use serde_json::json;

        // Structured array: weight + label honored, missing weight defaults to 1.
        let c = ProviderCredentials::from_json(&json!({
            "api_keys": [
                {"key": "sk-1", "weight": 3, "label": "primary"},
                {"key": "sk-2"}
            ]
        }));
        assert_eq!(c.api_keys.len(), 2);
        assert_eq!(c.api_keys[0].key, "sk-1");
        assert_eq!(c.api_keys[0].weight, 3);
        assert_eq!(c.api_keys[0].label.as_deref(), Some("primary"));
        assert_eq!(c.api_keys[1].key, "sk-2");
        assert_eq!(c.api_keys[1].weight, 1); // default
        assert!(c.api_keys[1].label.is_none());

        // Both a single api_key and a weighted pool may coexist.
        let c = ProviderCredentials::from_json(&json!({
            "api_key": "sk-fallback",
            "api_keys": [{"key": "sk-a", "weight": 2}]
        }));
        assert_eq!(c.api_key.as_deref(), Some("sk-fallback"));
        assert_eq!(c.api_keys.len(), 1);
        assert_eq!(c.api_keys[0].weight, 2);

        // Entries with an empty key are dropped (an empty bearer token is never valid).
        let c = ProviderCredentials::from_json(&json!({
            "api_keys": [{"key": ""}, {"key": "sk-real"}]
        }));
        assert_eq!(c.api_keys.len(), 1);
        assert_eq!(c.api_keys[0].key, "sk-real");

        // A malformed (non-array, non-string) api_keys value yields no entries.
        let c = ProviderCredentials::from_json(&json!({"api_keys": 5}));
        assert!(c.api_keys.is_empty());
    }

    #[test]
    fn from_json_parses_api_keys_env_string() {
        use serde_json::json;

        // The env-style "key:weight,key" string form is also accepted.
        let c = ProviderCredentials::from_json(&json!({
            "api_keys": "sk-1:3,sk-2"
        }));
        assert_eq!(c.api_keys.len(), 2);
        assert_eq!(c.api_keys[0].key, "sk-1");
        assert_eq!(c.api_keys[0].weight, 3);
        assert_eq!(c.api_keys[1].key, "sk-2");
        assert_eq!(c.api_keys[1].weight, 1);
    }

    #[test]
    fn from_json_parses_credential_sets() {
        use serde_json::json;

        // Signing-scheme weighted pool: key_id/key_secret/region + weight/label.
        let c = ProviderCredentials::from_json(&json!({
            "credential_sets": [
                {"key_id": "AKIA1", "key_secret": "s1", "region": "us-east-1", "weight": 3, "label": "prod"},
                {"key_id": "AKIA2", "key_secret": "s2"}
            ]
        }));
        assert_eq!(c.credential_sets.len(), 2);
        assert_eq!(c.credential_sets[0].key_id, "AKIA1");
        assert_eq!(c.credential_sets[0].key_secret, "s1");
        assert_eq!(c.credential_sets[0].region.as_deref(), Some("us-east-1"));
        assert_eq!(c.credential_sets[0].weight, 3);
        assert_eq!(c.credential_sets[1].key_id, "AKIA2");
        assert_eq!(c.credential_sets[1].weight, 1); // default
        assert!(c.credential_sets[1].region.is_none()); // optional

        // Entries missing key_id or key_secret are dropped (can't sign with them).
        let c = ProviderCredentials::from_json(&json!({
            "credential_sets": [
                {"key_id": "", "key_secret": "s"},
                {"key_id": "k", "key_secret": ""},
                {"key_id": "good", "key_secret": "ok"}
            ]
        }));
        assert_eq!(c.credential_sets.len(), 1);
        assert_eq!(c.credential_sets[0].key_id, "good");

        // A single signing credential still parses with no pool.
        let c = ProviderCredentials::from_json(&json!({
            "key_id": "AKIA", "key_secret": "shh", "region": "us"
        }));
        assert!(c.credential_sets.is_empty());
        assert_eq!(c.key_id.as_deref(), Some("AKIA"));
    }

    #[test]
    fn with_signing_overrides_credential_keeping_region_fallback() {
        use crate::types::CredentialEntry;

        let base = ProviderCredentials {
            key_id: Some("OLD".into()),
            key_secret: Some("old".into()),
            region: Some("us-west-2".into()),
            ..Default::default()
        };

        // Entry with its own region overrides everything.
        let c = base.with_signing(&CredentialEntry {
            key_id: "NEW".into(),
            key_secret: "new".into(),
            region: Some("eu-central-1".into()),
            weight: 1,
            label: None,
        });
        assert_eq!(c.key_id.as_deref(), Some("NEW"));
        assert_eq!(c.key_secret.as_deref(), Some("new"));
        assert_eq!(c.region.as_deref(), Some("eu-central-1"));

        // Entry without a region keeps the base/default region.
        let c = base.with_signing(&CredentialEntry {
            key_id: "NEW".into(),
            key_secret: "new".into(),
            region: None,
            weight: 1,
            label: None,
        });
        assert_eq!(c.region.as_deref(), Some("us-west-2"), "region falls back when entry omits it");
    }
}
