//! What litegen does with outputs: whether it keeps them, for how long, and
//! who may open them (`docs/storage-access.md`).
//!
//! Three layers decide it, each field on its own: the app's override (the
//! `app_file_access` table, `/v1/apps/{id}/access`), then the gateway setting
//! (`/v1/admin/settings`), then `storage_access` in `litegen.yaml`.

pub mod present;
pub mod signing;

use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::config::StorageAccessConfig;
use crate::db::DatabaseStore;
use crate::management::types::GatewaySettings;
use crate::types::AppFileAccessRow;

/// The shortest and longest life a signed link may have. A week is the most
/// an S3 presigned GET allows.
pub const MIN_SIGNED_URL_TTL_SECONDS: u32 = 60;
pub const MAX_SIGNED_URL_TTL_SECONDS: u32 = 604_800;

/// The longest `retention_days` may be: a hundred years. 0 keeps files until
/// they are deleted.
pub const MAX_RETENTION_DAYS: u32 = 36_500;

/// How the URLs litegen hands out for stored files work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AccessMode {
    /// The stored URL itself: anyone holding it can open the file.
    #[default]
    Public,
    /// A link that works without credentials until it expires.
    Signed,
    /// litegen's file route: only callers the owner scope allows.
    Private,
}

/// Which API keys pass the owner check for a stored file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum OwnerScope {
    /// Only the key that made it.
    #[default]
    Key,
    /// Any key of the same app.
    App,
    /// Any key of the same org.
    Org,
}

/// Who signs a signed link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SignedLinks {
    /// The store that holds the file: an S3 presigned GET, or litegen's local
    /// store's own link. A file no store can sign gets a litegen link. An S3
    /// presigned link can't be revoked.
    #[default]
    Store,
    /// litegen, for every file: a link to its file route, which can be revoked.
    Litegen,
}

impl SignedLinks {
    pub fn as_str(self) -> &'static str {
        match self {
            SignedLinks::Store => "store",
            SignedLinks::Litegen => "litegen",
        }
    }
}

impl std::str::FromStr for SignedLinks {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "store" => Ok(SignedLinks::Store),
            "litegen" => Ok(SignedLinks::Litegen),
            other => Err(format!("unknown signed_links {other:?}; expected store or litegen")),
        }
    }
}

impl AccessMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AccessMode::Public => "public",
            AccessMode::Signed => "signed",
            AccessMode::Private => "private",
        }
    }
}

impl std::str::FromStr for AccessMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "public" => Ok(AccessMode::Public),
            "signed" => Ok(AccessMode::Signed),
            "private" => Ok(AccessMode::Private),
            other => Err(format!("unknown access mode {other:?}; expected public, signed or private")),
        }
    }
}

impl OwnerScope {
    pub fn as_str(self) -> &'static str {
        match self {
            OwnerScope::Key => "key",
            OwnerScope::App => "app",
            OwnerScope::Org => "org",
        }
    }
}

impl std::str::FromStr for OwnerScope {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "key" => Ok(OwnerScope::Key),
            "app" => Ok(OwnerScope::App),
            "org" => Ok(OwnerScope::Org),
            other => Err(format!("unknown owner scope {other:?}; expected key, app or org")),
        }
    }
}

/// Whether a signed link may live this long.
pub fn ttl_in_range(seconds: i64) -> bool {
    (i64::from(MIN_SIGNED_URL_TTL_SECONDS)..=i64::from(MAX_SIGNED_URL_TTL_SECONDS)).contains(&seconds)
}

/// Whether `retention_days` may be this many days (0 keeps files until deleted).
pub fn retention_in_range(days: i64) -> bool {
    (0..=i64::from(MAX_RETENTION_DAYS)).contains(&days)
}

/// The rules in force for one app's outputs: whether they are kept, how long,
/// and how their links work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessPolicy {
    pub mode: AccessMode,
    pub owner_scope: OwnerScope,
    /// How long a signed link works.
    pub ttl: Duration,
    /// Whether litegen keeps a copy of each output. When it doesn't, answers
    /// carry the vendor's own link, or the image itself.
    pub keep_outputs: bool,
    /// Days after which kept files are deleted; 0 keeps them until deleted.
    pub retention_days: u32,
    /// Who signs a signed link.
    pub signed_links: SignedLinks,
}

/// The policy from the three layers. A stored value litegen does not know
/// (written by another version, or by hand) falls through to the next layer
/// rather than failing the request.
pub fn effective(defaults: &StorageAccessConfig, gateway: &GatewaySettings, app: Option<&AppFileAccessRow>) -> AccessPolicy {
    let app_mode = app.and_then(|r| r.mode.as_deref()).and_then(|m| m.parse().ok());
    let app_scope = app.and_then(|r| r.owner_scope.as_deref()).and_then(|s| s.parse().ok());
    let app_ttl = app.and_then(|r| r.signed_url_ttl_seconds).filter(|t| ttl_in_range(*t)).map(|t| t as u32);
    let gateway_ttl = gateway.storage_access_signed_url_ttl_seconds.filter(|t| ttl_in_range(i64::from(*t)));
    let app_retention = app.and_then(|r| r.retention_days).filter(|d| retention_in_range(*d)).map(|d| d as u32);
    let gateway_retention = gateway.storage_access_retention_days.filter(|d| *d <= MAX_RETENTION_DAYS);
    let app_links = app.and_then(|r| r.signed_links.as_deref()).and_then(|l| l.parse().ok());
    AccessPolicy {
        mode: app_mode.or(gateway.storage_access_mode).unwrap_or(defaults.mode),
        owner_scope: app_scope.or(gateway.storage_access_owner_scope).unwrap_or(defaults.owner_scope),
        ttl: Duration::from_secs(u64::from(app_ttl.or(gateway_ttl).unwrap_or(defaults.signed_url_ttl_seconds))),
        keep_outputs: app.and_then(|r| r.keep_outputs).or(gateway.storage_access_keep_outputs).unwrap_or(defaults.keep_outputs),
        retention_days: app_retention.or(gateway_retention).unwrap_or(defaults.retention_days),
        signed_links: app_links.or(gateway.storage_access_signed_links).unwrap_or(defaults.signed_links),
    }
}

#[cfg(test)]
thread_local! {
    static POLICY_LOOKUPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many policies this thread has resolved: what batch presenting saves.
#[cfg(test)]
pub(crate) fn policy_lookups() -> usize {
    POLICY_LOOKUPS.with(|c| c.get())
}

/// The policy for `app_id`'s files (none: the gateway's). A database error is
/// logged and answered with the layers above the app, never a failed request.
pub async fn resolve_policy(
    db: &dyn DatabaseStore,
    defaults: &StorageAccessConfig,
    gateway: &GatewaySettings,
    app_id: Option<&str>,
) -> AccessPolicy {
    #[cfg(test)]
    POLICY_LOOKUPS.with(|c| c.set(c.get() + 1));
    let row = match app_id {
        Some(app) => match db.get_app_file_access(app).await {
            Ok(row) => row,
            Err(e) => {
                tracing::error!(app_id = %app, error = %e, "could not read the app's file access settings; using the gateway's");
                None
            }
        },
        None => None,
    };
    effective(defaults, gateway, row.as_ref())
}


/// The stored-output settings in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AccessSettings {
    pub mode: AccessMode,
    pub owner_scope: OwnerScope,
    /// How long a signed link works, in seconds.
    pub signed_url_ttl_seconds: u32,
    /// Whether litegen keeps a copy of each output. `false`: answers carry
    /// the vendor's own link (video, 3D) or the image itself, and nothing is
    /// written.
    pub keep_outputs: bool,
    /// Days after which kept files are deleted; 0 keeps them until deleted.
    pub retention_days: u32,
    /// Who signs a signed link: `store` (the bucket, or litegen's local
    /// store) or `litegen` (revocable).
    pub signed_links: SignedLinks,
}

impl From<AccessPolicy> for AccessSettings {
    fn from(p: AccessPolicy) -> Self {
        Self {
            mode: p.mode,
            owner_scope: p.owner_scope,
            signed_url_ttl_seconds: p.ttl.as_secs() as u32,
            keep_outputs: p.keep_outputs,
            retention_days: p.retention_days,
            signed_links: p.signed_links,
        }
    }
}

/// One app's own settings; `null` uses the gateway's value.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AppAccessOverrides {
    pub mode: Option<AccessMode>,
    pub owner_scope: Option<OwnerScope>,
    pub signed_url_ttl_seconds: Option<u32>,
    pub keep_outputs: Option<bool>,
    pub retention_days: Option<u32>,
    pub signed_links: Option<SignedLinks>,
}

/// `PUT /v1/apps/{app_id}/access`: the app's settings, every one replaced.
/// An omitted or `null` field uses the gateway's value; `{}` clears them all.
#[derive(Debug, Clone, Default, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PutAppAccessRequest {
    /// `public`, `signed` or `private`.
    #[serde(default)]
    #[schema(value_type = Option<AccessMode>)]
    pub mode: Option<String>,
    /// `key`, `app` or `org`.
    #[serde(default)]
    #[schema(value_type = Option<OwnerScope>)]
    pub owner_scope: Option<String>,
    /// 60 to 604800.
    #[serde(default)]
    #[schema(value_type = Option<u32>)]
    pub signed_url_ttl_seconds: Option<i64>,
    /// `false` keeps nothing: answers carry the vendor's own link, or the
    /// image itself.
    #[serde(default)]
    #[schema(value_type = Option<bool>)]
    pub keep_outputs: Option<serde_json::Value>,
    /// 0 to 36500; 0 keeps files until they are deleted.
    #[serde(default)]
    #[schema(value_type = Option<u32>)]
    pub retention_days: Option<i64>,
    /// `store` or `litegen`.
    #[serde(default)]
    #[schema(value_type = Option<SignedLinks>)]
    pub signed_links: Option<String>,
}

/// `GET`/`PUT /v1/apps/{app_id}/access`: who may open the app's stored files.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AppAccessSettings {
    pub app_id: String,
    /// What the app sets itself.
    pub overrides: AppAccessOverrides,
    /// What its files follow: its own settings, else the gateway's, else `litegen.yaml`'s.
    pub effective: AccessSettings,
    /// When the app's own settings last changed; `null` when it has none.
    #[schema(value_type = Option<String>, format = DateTime)]
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
    /// litegen's links to the app's files issued before this time are refused:
    /// the latest of the app's revocation and the gateway's.
    #[schema(value_type = Option<String>, format = DateTime)]
    pub links_revoked_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl AppAccessOverrides {
    /// The overrides an app's row holds, skipping values this litegen does not know.
    pub fn of(row: Option<&AppFileAccessRow>) -> Self {
        Self {
            mode: row.and_then(|r| r.mode.as_deref()).and_then(|m| m.parse().ok()),
            owner_scope: row.and_then(|r| r.owner_scope.as_deref()).and_then(|s| s.parse().ok()),
            signed_url_ttl_seconds: row.and_then(|r| r.signed_url_ttl_seconds).filter(|t| ttl_in_range(*t)).map(|t| t as u32),
            keep_outputs: row.and_then(|r| r.keep_outputs),
            retention_days: row.and_then(|r| r.retention_days).filter(|d| retention_in_range(*d)).map(|d| d as u32),
            signed_links: row.and_then(|r| r.signed_links.as_deref()).and_then(|l| l.parse().ok()),
        }
    }
}

/// What a revocation answers: litegen's links issued before `revoked_at` no
/// longer work. A bucket's own presigned links aren't litegen's: they work
/// until they expire.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LinksRevoked {
    #[schema(value_type = String, format = DateTime)]
    pub revoked_at: chrono::DateTime<chrono::Utc>,
}

/// Now, to the millisecond: what a revocation records and what a link's
/// `issued` is compared with.
pub fn revocation_time() -> chrono::DateTime<chrono::Utc> {
    use chrono::SubsecRound;
    chrono::Utc::now().trunc_subsecs(3)
}

/// Whether a link issued at `issued` (unix ms; `None` for a link minted before
/// links carried their issue time) falls to a revocation at `revoked_at`.
pub fn link_is_revoked(issued: Option<i64>, revoked_at: Option<chrono::DateTime<chrono::Utc>>) -> bool {
    match (issued, revoked_at) {
        (_, None) => false,
        (None, Some(_)) => true,
        (Some(issued), Some(at)) => issued < at.timestamp_millis(),
    }
}

/// `X-Litegen-Keep-Outputs`: one request can keep less than its app does.
pub const KEEP_OUTPUTS_HEADER: &str = "x-litegen-keep-outputs";

/// What a request's `X-Litegen-Keep-Outputs` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepHeader {
    /// No header: the app's setting decides.
    Absent,
    /// `false`: keep nothing for this request.
    KeepNothing,
}

/// Read `X-Litegen-Keep-Outputs`. Its only value is `false`, in any case: a
/// request can keep less than its app does, never more.
pub fn keep_header(headers: &axum::http::HeaderMap) -> Result<KeepHeader, String> {
    let mut values = headers.get_all(KEEP_OUTPUTS_HEADER).iter();
    let Some(first) = values.next() else { return Ok(KeepHeader::Absent) };
    let only_false = values.next().is_none() && first.to_str().is_ok_and(|v| v.trim().eq_ignore_ascii_case("false"));
    if only_false {
        Ok(KeepHeader::KeepNothing)
    } else {
        Err("X-Litegen-Keep-Outputs takes only false: a request can keep less than its app does, never more".into())
    }
}

/// Whether this request's outputs are kept: the app keeps them and the
/// request didn't say otherwise.
pub fn keeps_outputs(policy: &AccessPolicy, header: KeepHeader) -> bool {
    policy.keep_outputs && header != KeepHeader::KeepNothing
}

/// Whether a generation row's outputs are kept: `metadata.keep_outputs`, which
/// is written only when it is `false`, at submit.
pub fn row_keeps_outputs(metadata: Option<&serde_json::Value>) -> bool {
    metadata.and_then(|m| m.get("keep_outputs")).and_then(|v| v.as_bool()) != Some(false)
}

/// [`resolve_policy`] with the gateway's config and settings.
pub async fn policy_for(state: &crate::api::middleware::AppState, app_id: Option<&str>) -> AccessPolicy {
    resolve_policy(&*state.db, &state.router.config.storage_access, &state.router.overrides().gateway, app_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StorageAccessConfig;
    use crate::management::types::GatewaySettings;
    use crate::types::AppFileAccessRow;
    use std::time::Duration;

    fn row(mode: Option<&str>, scope: Option<&str>, ttl: Option<i64>) -> AppFileAccessRow {
        AppFileAccessRow {
            app_id: "app".into(),
            mode: mode.map(str::to_string),
            owner_scope: scope.map(str::to_string),
            signed_url_ttl_seconds: ttl,
            keep_outputs: None,
            retention_days: None,
            signed_links: None,
            updated_at: chrono::Utc::now(),
        }
    }

    fn yaml(mode: AccessMode, owner_scope: OwnerScope, ttl: u32) -> StorageAccessConfig {
        StorageAccessConfig { mode, owner_scope, signed_url_ttl_seconds: ttl, ..Default::default() }
    }

    /// The new fields at their defaults: every output kept, never deleted,
    /// links signed by the store that holds the file.
    const KEEP_FOREVER: AccessPolicy = AccessPolicy {
        mode: AccessMode::Public,
        owner_scope: OwnerScope::Key,
        ttl: Duration::from_secs(3600),
        keep_outputs: true,
        retention_days: 0,
        signed_links: SignedLinks::Store,
    };

    #[test]
    fn with_nothing_set_files_are_public_to_the_key_for_an_hour() {
        let p = effective(&StorageAccessConfig::default(), &GatewaySettings::default(), None);
        assert_eq!(p, AccessPolicy { mode: AccessMode::Public, owner_scope: OwnerScope::Key, ttl: Duration::from_secs(3600), ..KEEP_FOREVER });
    }

    #[test]
    fn the_gateway_setting_overrides_the_yaml_field_by_field() {
        let gateway = GatewaySettings { storage_access_mode: Some(AccessMode::Signed), ..Default::default() };
        let p = effective(&yaml(AccessMode::Private, OwnerScope::App, 900), &gateway, None);
        assert_eq!(p, AccessPolicy { mode: AccessMode::Signed, owner_scope: OwnerScope::App, ttl: Duration::from_secs(900), ..KEEP_FOREVER });
    }

    #[test]
    fn an_app_override_wins_field_by_field() {
        let gateway = GatewaySettings {
            storage_access_owner_scope: Some(OwnerScope::Org),
            storage_access_signed_url_ttl_seconds: Some(120),
            ..Default::default()
        };
        let p = effective(&yaml(AccessMode::Signed, OwnerScope::App, 900), &gateway, Some(&row(Some("private"), None, Some(600))));
        assert_eq!(p, AccessPolicy { mode: AccessMode::Private, owner_scope: OwnerScope::Org, ttl: Duration::from_secs(600), ..KEEP_FOREVER });
    }

    #[test]
    fn an_unknown_stored_value_falls_back_to_the_next_layer() {
        // A value an older or newer litegen wrote, or one edited by hand: each
        // field falls through on its own rather than failing the request.
        let mut odd = row(Some("open"), Some("team"), Some(5));
        odd.retention_days = Some(-1);
        odd.signed_links = Some("edge".into());
        let p = effective(&yaml(AccessMode::Signed, OwnerScope::App, 900), &GatewaySettings::default(), Some(&odd));
        assert_eq!(p, AccessPolicy { mode: AccessMode::Signed, owner_scope: OwnerScope::App, ttl: Duration::from_secs(900), ..KEEP_FOREVER });
    }

    #[test]
    fn the_new_fields_layer_like_the_others() {
        let defaults = StorageAccessConfig::default();
        let gateway = GatewaySettings {
            storage_access_keep_outputs: Some(false),
            storage_access_retention_days: Some(30),
            ..Default::default()
        };
        let p = effective(&defaults, &gateway, None);
        assert!(!p.keep_outputs, "the gateway's false over the yaml's true");
        assert_eq!(p.retention_days, 30);
        assert_eq!(p.signed_links, SignedLinks::Store, "the yaml's");
        let mut own = row(None, None, None);
        own.keep_outputs = Some(true);
        own.retention_days = Some(7);
        own.signed_links = Some("litegen".into());
        let p = effective(&defaults, &gateway, Some(&own));
        assert!(p.keep_outputs, "the app's own true");
        assert_eq!(p.retention_days, 7);
        assert_eq!(p.signed_links, SignedLinks::Litegen);
        own.retention_days = Some(0);
        assert_eq!(effective(&defaults, &gateway, Some(&own)).retention_days, 0, "an app can keep forever under a gateway limit");
    }

    #[test]
    fn the_keep_header_takes_only_false_and_can_only_keep_less() {
        let headers = |vals: &[&str]| {
            let mut h = axum::http::HeaderMap::new();
            for v in vals {
                h.append(KEEP_OUTPUTS_HEADER, v.parse().unwrap());
            }
            h
        };
        assert_eq!(keep_header(&headers(&[])), Ok(KeepHeader::Absent));
        assert_eq!(keep_header(&headers(&["false"])), Ok(KeepHeader::KeepNothing));
        assert_eq!(keep_header(&headers(&[" FALSE "])), Ok(KeepHeader::KeepNothing));
        for bad in [&["true"][..], &["0"], &["no"], &["false", "false"]] {
            assert!(keep_header(&headers(bad)).is_err(), "{bad:?}");
        }
        let keeping = effective(&StorageAccessConfig::default(), &GatewaySettings::default(), None);
        let nothing = AccessPolicy { keep_outputs: false, ..keeping };
        assert!(keeps_outputs(&keeping, KeepHeader::Absent));
        assert!(!keeps_outputs(&keeping, KeepHeader::KeepNothing));
        assert!(!keeps_outputs(&nothing, KeepHeader::Absent), "the app's choice stands without the header");
        assert!(row_keeps_outputs(None) && row_keeps_outputs(Some(&serde_json::json!({}))));
        assert!(!row_keeps_outputs(Some(&serde_json::json!({ "keep_outputs": false }))));
    }

    #[test]
    fn modes_and_scopes_spell_as_the_docs_do() {
        for (m, s) in [(AccessMode::Public, "public"), (AccessMode::Signed, "signed"), (AccessMode::Private, "private")] {
            assert_eq!(m.as_str(), s);
            assert_eq!(s.parse::<AccessMode>(), Ok(m));
            assert_eq!(serde_json::to_value(m).unwrap(), serde_json::json!(s));
        }
        for (o, s) in [(OwnerScope::Key, "key"), (OwnerScope::App, "app"), (OwnerScope::Org, "org")] {
            assert_eq!(o.as_str(), s);
            assert_eq!(s.parse::<OwnerScope>(), Ok(o));
        }
        assert!("Private".parse::<AccessMode>().is_err());
        for (l, s) in [(SignedLinks::Store, "store"), (SignedLinks::Litegen, "litegen")] {
            assert_eq!(l.as_str(), s);
            assert_eq!(s.parse::<SignedLinks>(), Ok(l));
            assert_eq!(serde_json::to_value(l).unwrap(), serde_json::json!(s));
        }
        assert!("Litegen".parse::<SignedLinks>().is_err());
    }

    #[tokio::test]
    async fn resolving_reads_the_apps_row_and_survives_a_missing_one() {
        let db = crate::db::sqlite::SqliteDatabase::connect("sqlite::memory:").await.unwrap();
        let defaults = yaml(AccessMode::Signed, OwnerScope::Key, 900);
        let gateway = GatewaySettings::default();
        let app = crate::api::middleware::DEFAULT_APP_ID;
        assert_eq!(resolve_policy(&db, &defaults, &gateway, Some(app)).await.mode, AccessMode::Signed);
        assert_eq!(resolve_policy(&db, &defaults, &gateway, None).await.mode, AccessMode::Signed);
        crate::db::DatabaseStore::put_app_file_access(&db, &crate::types::AppFileAccessUpsert {
            app_id: app.into(),
            mode: Some("private".into()),
            owner_scope: None,
            signed_url_ttl_seconds: None,
            keep_outputs: None,
            retention_days: None,
            signed_links: None,
        })
        .await
        .unwrap();
        assert_eq!(resolve_policy(&db, &defaults, &gateway, Some(app)).await.mode, AccessMode::Private);
        assert_eq!(resolve_policy(&db, &defaults, &gateway, Some("no-such-app")).await.mode, AccessMode::Signed);
    }
}
