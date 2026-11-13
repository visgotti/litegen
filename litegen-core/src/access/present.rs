//! The URLs litegen hands out for stored files, minted when it answers
//! (`docs/storage-access.md`). Rows keep the URL the file was stored at;
//! every answer and webhook passes it through a [`Presenter`], so a change of
//! mode applies to files made before it, and signed links are fresh on every
//! read.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{resolve_policy, AccessMode, AccessPolicy, OwnerScope};
use crate::access::signing::LinkSigner;
use crate::api::middleware::{AppState, KeyContext};
use crate::db::DatabaseStore;
use crate::proxy::router::ProxyRouter;
use crate::proxy::storage::ImageStorage;
use crate::types::{Generation, GenerationAsset, Model3dGenerationResponse, VideoGenerationResponse};

/// Who made a stored file: its generation, and that generation's key, app and org.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileOwner {
    pub generation_id: String,
    pub key_id: Option<Uuid>,
    pub app_id: Option<String>,
    pub org_id: Option<String>,
}

impl FileOwner {
    pub fn of(gen: &Generation) -> Self {
        Self { generation_id: gen.id.clone(), key_id: gen.key_id, app_id: gen.app_id.clone(), org_id: gen.org_id.clone() }
    }

    /// A generation the caller of this request is making.
    pub fn made_by(generation_id: &str, ctx: Option<&KeyContext>) -> Self {
        Self {
            generation_id: generation_id.to_string(),
            key_id: ctx.and_then(|c| c.key_id),
            app_id: ctx.and_then(|c| c.app_id.clone()),
            org_id: ctx.and_then(|c| c.org_id.clone()),
        }
    }
}

/// Whether `viewer` passes the owner check for `owner`'s files.
///
/// The master key (and no auth at all) always does. So does a dashboard
/// session: the route has already held it to its `generation:read` rights.
/// An API key passes when it is the key that made the file (`key`), or of the
/// same app (`app`) or org (`org`).
pub fn in_owner_scope(scope: OwnerScope, viewer: Option<&KeyContext>, owner: &FileOwner) -> bool {
    let Some(ctx) = viewer else { return true };
    if ctx.user.is_some() || crate::api::middleware::is_platform_admin(ctx) {
        return true;
    }
    match scope {
        OwnerScope::Key => ctx.key_id.is_some() && ctx.key_id == owner.key_id,
        OwnerScope::App => ctx.app_id.is_some() && ctx.app_id == owner.app_id,
        OwnerScope::Org => ctx.org_id.is_some() && ctx.org_id == owner.org_id,
    }
}

/// Who an answer is for.
#[derive(Debug, Clone, Copy)]
pub enum Viewer<'a> {
    /// The caller of this request.
    Caller(Option<&'a KeyContext>),
    /// The generation's own key: its webhook.
    Owner,
}

/// What presenting needs. Built from the request state, or handed to the
/// poller, which has none.
#[derive(Clone)]
pub struct Presenting {
    pub router: Arc<ProxyRouter>,
    pub db: Arc<dyn DatabaseStore>,
    pub secrets_key: Option<[u8; 32]>,
    /// The deployment's tenancy: in hosted mode an app's own bucket is a
    /// tenant's (`handlers::TenantBucket`).
    pub mode: crate::config::Mode,
}

impl Presenting {
    pub fn of(state: &AppState) -> Self {
        Self { router: state.router.clone(), db: state.db.clone(), secrets_key: state.secrets_key, mode: state.mode }
    }

    /// Whose this deployment's app buckets are.
    pub(crate) fn tenant_bucket(&self) -> crate::api::handlers::TenantBucket {
        crate::api::handlers::tenant_bucket(self.mode, Some(self.router.config.as_ref()))
    }

    /// How `owner`'s files are handed to `viewer`.
    pub async fn presenter(&self, owner: &FileOwner, viewer: Viewer<'_>) -> Presenter {
        self.presenter_cached(owner, viewer, &mut PresenterCache::default()).await
    }

    /// [`presenter`](Self::presenter), with each app's policy and stores
    /// resolved once across the calls that share `cache` (a page of rows).
    pub async fn presenter_cached(&self, owner: &FileOwner, viewer: Viewer<'_>, cache: &mut PresenterCache) -> Presenter {
        let entry = match cache.apps.entry(owner.app_id.clone()) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => {
                let policy = resolve_policy(
                    &*self.db,
                    &self.router.config.storage_access,
                    &self.router.overrides().gateway,
                    owner.app_id.as_deref(),
                )
                .await;
                v.insert(AppPresenting { policy, stores: None })
            }
        };
        let policy = entry.policy;
        let in_scope = match viewer {
            Viewer::Owner => true,
            Viewer::Caller(ctx) => in_owner_scope(policy.owner_scope, ctx, owner),
        };
        // Only a signed link needs the store that holds the file.
        let stores = if policy.mode == AccessMode::Signed && in_scope {
            if entry.stores.is_none() {
                entry.stores = Some(self.stores(owner.app_id.as_deref()).await);
            }
            entry.stores.clone().unwrap_or_default()
        } else {
            Vec::new()
        };
        Presenter {
            policy,
            in_scope,
            stores,
            signer: self.router.link_signer.clone(),
            base_url: self.router.config.server.public_base_url(),
            generation_id: owner.generation_id.clone(),
        }
    }

    /// The stores a file of this app can be in: its own bucket first, then litegen's.
    async fn stores(&self, app_id: Option<&str>) -> Vec<Arc<dyn ImageStorage>> {
        let mut stores = vec![self.router.model3d_store.clone()];
        if let Some(app) = app_id {
            if let Some((store, _)) = crate::api::handlers::resolve_app_model3d_store(&self.db, self.secrets_key, &self.router.config.aws, self.tenant_bucket(), app).await {
                stores.insert(0, store);
            }
        }
        stores
    }
}

/// What presenting a page of rows resolves once per app.
#[derive(Default)]
pub struct PresenterCache {
    apps: std::collections::HashMap<Option<String>, AppPresenting>,
}

struct AppPresenting {
    policy: AccessPolicy,
    stores: Option<Vec<Arc<dyn ImageStorage>>>,
}

/// Turns the URLs one generation's files were stored at into the ones handed out.
pub struct Presenter {
    policy: AccessPolicy,
    in_scope: bool,
    stores: Vec<Arc<dyn ImageStorage>>,
    signer: Arc<LinkSigner>,
    base_url: String,
    generation_id: String,
}

impl Presenter {
    pub fn policy(&self) -> AccessPolicy {
        self.policy
    }

    /// litegen's file route for the generation's file at `index`.
    pub fn file_route(&self, index: usize) -> String {
        format!("{}/v1/generations/{}/assets/{index}", self.base_url, self.generation_id)
    }

    /// litegen's own signed link to the file at `index`, which works without
    /// credentials until it expires and can be revoked.
    pub fn litegen_link(&self, index: usize) -> (String, DateTime<Utc>) {
        let now = Utc::now();
        let expires = now + chrono::Duration::from_std(self.policy.ttl).unwrap_or_else(|_| chrono::Duration::hours(1));
        let query = self.signer.file_route_query(&self.generation_id, index, now.timestamp_millis(), expires.timestamp());
        (format!("{}/v1/generations/{}/assets/{index}/signed?{query}", self.base_url, self.generation_id), expires)
    }

    /// The URL to hand out for the file stored at `stored`, which
    /// `GET /v1/generations/{id}/assets/{index}` streams, and when it stops
    /// working (signed links only).
    ///
    /// - `public`: the stored URL.
    /// - `private`, or `signed` for a caller outside the owner scope: the file
    ///   route, which checks who is asking.
    /// - `signed`: a link signed by the store that holds the file, or by
    ///   litegen (`signed_links: litegen`, or a file no store can sign, such as
    ///   a vendor's own file).
    ///
    /// A URL with no index is not one the file route serves, and is handed
    /// out as stored: in a non-public mode the bucket is private and the
    /// asset route refuses it, so it opens nothing.
    pub async fn url(&self, index: Option<usize>, stored: &str) -> (String, Option<DateTime<Utc>>) {
        let route = || index.map_or_else(|| stored.to_string(), |i| self.file_route(i));
        match self.policy.mode {
            AccessMode::Public => (stored.to_string(), None),
            AccessMode::Private => (route(), None),
            AccessMode::Signed if !self.in_scope => (route(), None),
            AccessMode::Signed => {
                if self.policy.signed_links == super::SignedLinks::Store {
                    for store in &self.stores {
                        if let Some(key) = store.key_of(stored) {
                            if let Some((url, expires)) = store.signed_url(&key, self.policy.ttl, &*self.signer).await {
                                return (url, Some(expires));
                            }
                        }
                    }
                }
                match index {
                    Some(i) => {
                        let (url, expires) = self.litegen_link(i);
                        (url, Some(expires))
                    }
                    None => (stored.to_string(), None),
                }
            }
        }
    }

    /// A generation row as handed out: `metadata.assets[].url` (with
    /// `expires_at` on signed links) and `result_url`. Each file's index is
    /// its position in what `GET /v1/generations/{id}/assets` lists.
    pub async fn present_row(&self, gen: &mut Generation) {
        if self.policy.mode == AccessMode::Public {
            return;
        }
        let served = crate::api::handlers::generation_assets(gen);
        let index_of = |url: &str| served.iter().position(|a| a.url == url);
        if let Some(assets) = gen.metadata.as_mut().and_then(|m| m.get_mut("assets")).and_then(|a| a.as_array_mut()) {
            for asset in assets.iter_mut() {
                let Some(stored) = asset.get("url").and_then(|u| u.as_str()).map(str::to_string) else { continue };
                let (url, expires) = self.url(index_of(&stored), &stored).await;
                if let Some(obj) = asset.as_object_mut() {
                    obj.insert("url".into(), url.into());
                    match expires {
                        Some(at) => obj.insert("expires_at".into(), serde_json::to_value(at).unwrap_or_default()),
                        None => obj.remove("expires_at"),
                    };
                }
            }
        }
        if let Some(stored) = gen.result_url.clone() {
            gen.result_url = Some(self.url(index_of(&stored), &stored).await.0);
        }
    }

    /// A 3D answer's assets as handed out; each one's index is its position.
    pub async fn present_3d(&self, resp: &mut Model3dGenerationResponse) {
        for (i, asset) in resp.assets.iter_mut().enumerate() {
            let (url, expires) = self.url(Some(i), &asset.url).await;
            asset.url = url;
            asset.expires_at = expires;
        }
    }

    /// A list of a generation's files as handed out.
    pub async fn present_assets(&self, assets: &mut [GenerationAsset]) {
        for asset in assets.iter_mut() {
            let (url, expires) = self.url(Some(asset.index as usize), &asset.url).await;
            asset.url = url;
            asset.expires_at = expires;
        }
    }

    /// A video answer's `video_url` as handed out. A video generation keeps
    /// one file, its video: the file at `index` among `assets`, when it is
    /// listed there, else the first.
    pub async fn present_video(&self, resp: &mut VideoGenerationResponse, assets: &[GenerationAsset]) {
        let Some(stored) = resp.video_url.clone() else { return };
        let index = assets.iter().position(|a| a.url == stored).unwrap_or(0);
        let (url, expires) = self.url(Some(index), &stored).await;
        resp.video_url = Some(url);
        resp.video_url_expires_at = expires;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::GenerationStatus;

    fn ctx(key: Option<Uuid>, app: Option<&str>, org: Option<&str>) -> KeyContext {
        KeyContext {
            key_id: key,
            scopes: Vec::new(),
            rpm_limit: None,
            quota_remaining: None,
            webhook_url: None,
            user: None,
            permissions: Vec::new(),
            session_id: None,
            org_id: org.map(str::to_string),
            app_id: app.map(str::to_string),
            oauth_grant_id: None,
        }
    }

    #[test]
    fn the_owner_scope_decides_which_keys_pass() {
        let (maker, sibling, stranger) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let owner = FileOwner { generation_id: "g".into(), key_id: Some(maker), app_id: Some("a".into()), org_id: Some("o".into()) };
        let maker_ctx = ctx(Some(maker), Some("a"), Some("o"));
        let same_app = ctx(Some(sibling), Some("a"), Some("o"));
        let same_org = ctx(Some(stranger), Some("b"), Some("o"));
        let other_org = ctx(Some(stranger), Some("c"), Some("p"));
        for (scope, expect) in [
            (OwnerScope::Key, [true, false, false, false]),
            (OwnerScope::App, [true, true, false, false]),
            (OwnerScope::Org, [true, true, true, false]),
        ] {
            let got = [&maker_ctx, &same_app, &same_org, &other_org].map(|c| in_owner_scope(scope, Some(c), &owner));
            assert_eq!(got, expect, "{scope:?}");
        }
        // The master key (no key, no session) and no auth at all always pass.
        assert!(in_owner_scope(OwnerScope::Key, Some(&ctx(None, None, None)), &owner));
        assert!(in_owner_scope(OwnerScope::Key, None, &owner));
        // A dashboard session: the route already held it to its read rights.
        let session = KeyContext {
            user: Some(crate::api::middleware::UserContext {
                user_id: "u".into(),
                email: "u@example.com".into(),
                role: crate::types::Role::Viewer,
            }),
            session_id: Some("s".into()),
            ..ctx(None, None, Some("o"))
        };
        for scope in [OwnerScope::Key, OwnerScope::App, OwnerScope::Org] {
            assert!(in_owner_scope(scope, Some(&session), &owner), "{scope:?}");
        }
        // A file made through the dashboard (no key) is no API key's under `key`.
        let session_made = FileOwner { key_id: None, ..owner.clone() };
        assert!(!in_owner_scope(OwnerScope::Key, Some(&maker_ctx), &session_made));
    }

    fn presenter(mode: AccessMode) -> Presenter {
        Presenter {
            policy: AccessPolicy {
                mode,
                owner_scope: OwnerScope::Key,
                ttl: std::time::Duration::from_secs(60),
                keep_outputs: true,
                retention_days: 0,
                signed_links: crate::access::SignedLinks::Store,
            },
            in_scope: true,
            stores: Vec::new(),
            signer: Arc::new(LinkSigner::from_secrets_key(None)),
            base_url: "https://gw.example".into(),
            generation_id: "g1".into(),
        }
    }

    #[tokio::test]
    async fn the_file_route_index_is_the_position_the_assets_route_streams() {
        // An image result whose second image could not be kept: the recorded
        // entries carry the indexes 0 and 2, and the file route serves them as
        // positions 0 and 1.
        let mut gen = Generation {
            id: "g1".into(),
            key_id: None,
            model: "m".into(),
            provider: "p".into(),
            media_type: "image".into(),
            status: GenerationStatus::Completed,
            progress: 100,
            provider_job_id: None,
            result_url: Some("https://b.example/c.png".into()),
            error_message: None,
            cost_usd: 0.0,
            created_at: Utc::now(),
            completed_at: None,
            metadata: Some(serde_json::json!({ "assets": [
                { "index": 0, "kind": "image", "format": "png", "url": "https://b.example/a.png" },
                { "index": 2, "kind": "image", "format": "png", "url": "https://b.example/c.png" },
            ]})),
            org_id: None,
            app_id: None,
            files_deleted_at: None,
            files_deleted_reason: None,
        };
        presenter(AccessMode::Private).present_row(&mut gen).await;
        let assets = gen.metadata.as_ref().unwrap()["assets"].as_array().unwrap();
        assert_eq!(assets[0]["url"], "https://gw.example/v1/generations/g1/assets/0");
        assert_eq!(assets[1]["url"], "https://gw.example/v1/generations/g1/assets/1");
        assert_eq!(gen.result_url.as_deref(), Some("https://gw.example/v1/generations/g1/assets/1"));
    }

    #[tokio::test]
    async fn presenting_many_rows_of_one_app_reads_its_policy_once() {
        let db: Arc<dyn DatabaseStore> = Arc::new(crate::db::sqlite::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let config = Arc::new(crate::config::AppConfig::default());
        let router = Arc::new(ProxyRouter::new(
            Arc::new(crate::proxy::registry::ProviderRegistry::new()),
            Arc::new(crate::proxy::cache::GenerationCache::new(&Default::default())),
            config,
            Arc::new(crate::proxy::storage::LocalStore),
        ));
        let presenting = Presenting { router, db, secrets_key: None, mode: crate::config::Mode::default() };
        let rows: Vec<FileOwner> = (0..10)
            .map(|i| FileOwner { generation_id: format!("g{i}"), key_id: None, app_id: Some("app-1".into()), org_id: Some("org".into()) })
            .collect();
        let before = super::super::policy_lookups();
        let mut cache = PresenterCache::default();
        for owner in &rows {
            presenting.presenter_cached(owner, Viewer::Owner, &mut cache).await;
        }
        assert_eq!(super::super::policy_lookups() - before, 1, "one app, one read of its policy");
    }

    #[tokio::test]
    async fn a_signed_link_no_store_can_make_is_litegens_own() {
        // A URL no configured store minted (a vendor's, or a bucket since
        // removed) can't be signed by a store: litegen signs a link to it,
        // which works without credentials as a signed link should.
        let (url, expires) = presenter(AccessMode::Signed).url(Some(3), "https://vendor.example/x.glb").await;
        assert!(url.starts_with("https://gw.example/v1/generations/g1/assets/3/signed?expires="), "{url}");
        assert!(expires.is_some_and(|e| e > Utc::now()), "it says when it stops working");
        let (url, expires) = presenter(AccessMode::Signed).url(None, "https://vendor.example/x.glb").await;
        assert_eq!((url.as_str(), expires), ("https://vendor.example/x.glb", None), "no index: as stored");
        let (url, _) = presenter(AccessMode::Public).url(Some(3), "https://vendor.example/x.glb").await;
        assert_eq!(url, "https://vendor.example/x.glb");
    }
}
