use async_trait::async_trait;
use uuid::Uuid;

use crate::types::*;

/// Where the person who made an API key stands in the key's org.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOwner {
    /// Nobody made it: a key made with the master key belongs to its org.
    None,
    /// Its maker is an active member of the key's org, with this role.
    Member(Role),
    /// Its maker is deactivated, gone, or no longer a member of the key's
    /// org (or holds a role litegen can't read): the key no longer works.
    Gone,
}

/// [`DatabaseStore::lookup_api_key_with_owner`] by its three lookups: the
/// key, its maker, and the maker's membership. The trait's default, and what
/// the one-statement overrides must answer alike.
pub async fn key_owner_by_lookups<D: DatabaseStore + ?Sized>(db: &D, key_hash: &str, null_org: Option<&str>) -> Result<Option<(ApiKey, KeyOwner)>, sqlx::Error> {
    let Some(key) = db.lookup_api_key_by_hash(key_hash).await? else { return Ok(None) };
    let owner = match key.owner_user_id.as_deref() {
        None => KeyOwner::None,
        Some(user_id) => match key.org_id.as_deref().or(null_org) {
            None => KeyOwner::Gone,
            Some(org_id) => match db.get_user_by_id(user_id).await? {
                Some(user) if user.is_active => db.get_membership(org_id, user_id).await?.map_or(KeyOwner::Gone, KeyOwner::Member),
                _ => KeyOwner::Gone,
            },
        },
    };
    Ok(Some((key, owner)))
}

/// Review 2026-11-11 P14: `db`'s one-statement reads answer as the lookups
/// they replace did, case by case. Shared by the SQLite and Postgres tests;
/// `tag` keeps one run's rows apart on a shared database, and `clear_org`
/// sets a key's `org_id` to NULL (no API makes such a key today).
#[cfg(test)]
pub(crate) async fn one_statement_reads_answer_as_the_lookups(db: &dyn DatabaseStore, tag: &str, clear_org: impl AsyncFn(&str)) {
    use crate::api::middleware::DEFAULT_ORG_ID as ORG;
    let now = chrono::Utc::now();
    let id = |what: &str| format!("{what}-{tag}");
    let person = |who: &str| User {
        id: id(who),
        email: format!("{}@example.com", id(who)),
        password_hash: None,
        role: Role::Member,
        oauth_github_id: None,
        oauth_google_id: None,
        created_at: now,
        updated_at: now,
        last_login_at: None,
        is_active: true,
        sign_in_epoch: 0,
    };
    // An app of its own, so its access row is this run's.
    let app = Application { id: id("app"), org_id: ORG.into(), name: id("app"), slug: id("app"), status: "active".into(), created_at: now, updated_at: now };
    db.create_application(&app).await.unwrap();
    for (who, role) in [("admin", Role::Admin), ("viewer", Role::Viewer), ("inactive", Role::Member), ("removed", Role::Member)] {
        db.create_user(&person(who)).await.unwrap();
        db.add_org_member(ORG, &id(who), role).await.unwrap();
    }
    db.update_user(&id("inactive"), None, Some(false), None).await.unwrap();
    db.remove_org_member(ORG, &id("removed")).await.unwrap();

    // The keys: (name, maker), each in the default org and this run's app.
    let mut hashes = Vec::new();
    for (name, maker) in [("none", None), ("admin", Some("admin")), ("viewer", Some("viewer")), ("inactive", Some("inactive")), ("removed", Some("removed")), ("nullorg", Some("admin")), ("revoked", Some("admin"))] {
        let hash = id(&format!("hash-{name}"));
        let key = db
            .create_api_key_scoped(ORG, &app.id, &id(&format!("pub-{name}")), name, &hash, "lg-x", None, None, "generate,read,admin", None, None)
            .await
            .unwrap();
        if let Some(maker) = maker {
            db.set_api_key_owner(&key.id, &id(maker)).await.unwrap();
        }
        if name == "nullorg" {
            clear_org(&key.id.to_string()).await;
        }
        if name == "revoked" {
            db.revoke_api_key(&key.id).await.unwrap();
        }
        hashes.push((name, hash));
    }
    let owner_of = |found: Option<(ApiKey, KeyOwner)>| found.map(|(k, o)| (k.id, k.org_id, o));
    for (name, hash) in &hashes {
        for null_org in [Some(ORG), None] {
            let joined = owner_of(db.lookup_api_key_with_owner(hash, null_org).await.unwrap());
            let looked = owner_of(key_owner_by_lookups(db, hash, null_org).await.unwrap());
            assert_eq!(joined, looked, "{name}, a key with no org counting as {null_org:?}'s");
            let want = match (*name, null_org) {
                ("revoked", _) => None,
                ("none", _) => Some(KeyOwner::None),
                ("admin", _) | ("nullorg", Some(_)) => Some(KeyOwner::Member(Role::Admin)),
                ("viewer", _) => Some(KeyOwner::Member(Role::Viewer)),
                _ => Some(KeyOwner::Gone),
            };
            assert_eq!(joined.map(|(_, _, o)| o), want, "{name}, {null_org:?}");
        }
    }
    assert!(db.lookup_api_key_with_owner(&id("no-such-hash"), Some(ORG)).await.unwrap().is_none());

    // A session and its user.
    let session = Session { id: id("session"), user_id: id("admin"), created_at: now, expires_at: now + chrono::Duration::days(7), ip: None, user_agent: None, csrf_token: id("csrf") };
    db.create_session(&session).await.unwrap();
    let (got, user) = db.get_session_with_user(&session.id).await.unwrap().expect("the session");
    let looked = db.get_session(&crate::auth::tokens::token_hash(&session.id)).await.unwrap().expect("the session, by its stored id");
    let looked_user = db.get_user_by_id(&looked.user_id).await.unwrap();
    assert_eq!((got.id, got.user_id, got.expires_at.timestamp(), got.csrf_token), (looked.id, looked.user_id, looked.expires_at.timestamp(), looked.csrf_token));
    let fields = |u: Option<User>| u.map(|u| (u.id, u.email, u.role, u.is_active, u.created_at.timestamp()));
    assert_eq!(fields(user), fields(looked_user));
    assert!(db.get_session_with_user(&id("no-such-session")).await.unwrap().is_none());

    // A generation's app and that app's access settings.
    for (gen, app_id) in [("gen-app", Some(app.id.as_str())), ("gen-none", None)] {
        db.insert_generation(&id(gen), None, "mock/mesh-3d", "mock", "model3d", Some("j"), 0.0, Some(ORG), app_id).await.unwrap();
    }
    let access = |found: Option<(Option<String>, Option<crate::types::AppFileAccessRow>)>| {
        found.map(|(a, row)| (a, row.map(|r| (r.app_id, r.mode, r.owner_scope, r.signed_url_ttl_seconds, r.keep_outputs, r.retention_days, r.signed_links))))
    };
    let lookups = async |gen: &str| -> Option<(Option<String>, Option<crate::types::AppFileAccessRow>)> {
        let g = db.get_generation(gen).await.unwrap()?;
        let row = match g.app_id.as_deref() {
            Some(a) => db.get_app_file_access(a).await.unwrap(),
            None => None,
        };
        Some((g.app_id, row))
    };
    for stage in ["no row", "a row"] {
        if stage == "a row" {
            db.put_app_file_access(&crate::types::AppFileAccessUpsert {
                app_id: app.id.clone(),
                mode: Some("signed".into()),
                owner_scope: Some("app".into()),
                signed_url_ttl_seconds: Some(120),
                keep_outputs: Some(false),
                retention_days: Some(30),
                signed_links: Some("litegen".into()),
            })
            .await
            .unwrap();
        }
        for gen in ["gen-app", "gen-none", "no-such-gen"] {
            let joined = access(db.generation_file_access(&id(gen)).await.unwrap());
            assert_eq!(joined, access(lookups(&id(gen)).await), "{gen}, {stage}");
        }
    }
    let (_, row) = db.generation_file_access(&id("gen-app")).await.unwrap().unwrap();
    assert_eq!(row.and_then(|r| r.mode).as_deref(), Some("signed"));
}

/// Sessions are kept by their token's hash (migration 044), so a copy of the
/// database holds no session that works. `create_session` and
/// `create_session_at_epoch` store the `token_hash` of the token they're
/// handed; `get_session_with_user` takes the cookie (the token) and hands
/// the session back with its stored id; every other session method takes
/// that stored id as it is. The stored id, sent as the cookie, finds
/// nothing. Shared by the SQLite and Postgres tests: `tag` keeps one run's
/// rows apart on a shared database, and `stored_ids` reads a user's
/// `sessions.id` values straight from the database, sorted.
#[cfg(test)]
pub(crate) async fn sessions_are_kept_by_their_tokens_hash(db: &dyn DatabaseStore, tag: &str, stored_ids: impl AsyncFn(&str) -> Vec<String>) {
    use crate::auth::tokens::token_hash;
    let now = chrono::Utc::now();
    let user = User {
        id: format!("hashed-sessions-{tag}"),
        email: format!("hashed-sessions-{tag}@example.com"),
        password_hash: Some("verified".into()),
        role: Role::Admin,
        oauth_github_id: None,
        oauth_google_id: None,
        created_at: now,
        updated_at: now,
        last_login_at: None,
        is_active: true,
        sign_in_epoch: 0,
    };
    db.create_user(&user).await.unwrap();
    let session = |token: &str| Session {
        id: token.to_string(),
        user_id: user.id.clone(),
        created_at: now,
        expires_at: now + chrono::Duration::days(7),
        ip: None,
        user_agent: None,
        csrf_token: format!("csrf-{token}"),
    };
    let (first, second) = (format!("first-{tag}"), format!("second-{tag}"));
    db.create_session(&session(&first)).await.unwrap();
    let epoch = db.sign_in_epoch(&user.id).await.unwrap().expect("the account");
    db.create_session_at_epoch(&session(&second), epoch).await.unwrap();
    let (first_id, second_id) = (token_hash(&first), token_hash(&second));
    let mut both = vec![first_id.clone(), second_id.clone()];
    both.sort();
    assert_eq!(stored_ids(&user.id).await, both, "the database keeps each token's hash, never the token");

    // The cookie finds its session, handed back with its stored id. The
    // stored id, sent as the cookie, finds nothing.
    for (token, id) in [(&first, &first_id), (&second, &second_id)] {
        let (found, owner) = db.get_session_with_user(token).await.unwrap().expect("the cookie finds its session");
        assert_eq!((found.id.as_str(), found.csrf_token), (id.as_str(), format!("csrf-{token}")), "{token}");
        assert_eq!(owner.map(|u| u.id), Some(user.id.clone()), "{token}");
        assert!(db.get_session_with_user(id).await.unwrap().is_none(), "{token}: a stored id is no cookie");
    }

    // Every other session method takes the stored id as it is.
    assert!(db.get_session(&first_id).await.unwrap().is_some(), "get_session by the stored id");
    assert!(db.get_session(&first).await.unwrap().is_none(), "get_session doesn't take the cookie");
    assert_eq!(db.session_epoch(&first_id).await.unwrap(), Some(epoch));
    let later = now + chrono::Duration::days(14);
    db.bump_session_expiry(&first_id, later).await.unwrap();
    assert_eq!(db.get_session(&first_id).await.unwrap().map(|s| s.expires_at.timestamp()), Some(later.timestamp()), "bumped by the stored id");
    let mut listed: Vec<String> = db.list_user_sessions(&user.id).await.unwrap().into_iter().map(|s| s.id).collect();
    listed.sort();
    assert_eq!(listed, both, "listed by their stored ids");
    let invitation = |what: &str| Invitation {
        id: format!("{what}-{tag}"),
        email: format!("{what}-{tag}@example.com"),
        role: Role::Member,
        token: format!("{what}-{tag}"),
        invited_by: Some(user.id.clone()),
        org_id: crate::api::middleware::DEFAULT_ORG_ID.to_string(),
        expires_at: now + chrono::Duration::days(7),
        used_at: None,
        created_at: now,
    };
    assert!(db.create_invitation_from(&invitation("from-the-stored-id"), SignIn::Session(&first_id)).await.unwrap(), "a session's stored id is its sign-in");
    assert!(!db.create_invitation_from(&invitation("from-the-cookie"), SignIn::Session(&first)).await.unwrap(), "its cookie is not");

    // A password change keeps the session it names by its stored id, and
    // ends the other one; deleting by the stored id ends the last.
    assert_eq!(db.change_own_password(&user.id, "changed", "verified", &first_id).await.unwrap(), PasswordChange::Done);
    assert_eq!(stored_ids(&user.id).await, [first_id.as_str()], "the other session ended");
    assert!(db.get_session_with_user(&first).await.unwrap().is_some(), "the kept session still signs in");
    db.create_session(&session(&second)).await.unwrap();
    assert_eq!(db.delete_user_sessions(&user.id, Some(&first_id)).await.unwrap(), 1, "all but the stored id named");
    db.delete_session(&first_id).await.unwrap();
    assert!(stored_ids(&user.id).await.is_empty(), "deleted by the stored id");
}

/// What a joined lookup read about a key's maker (`users.is_active` and
/// `organization_members.role`, both NULL when nothing matched), as
/// [`key_owner_by_lookups`] judges it.
pub(crate) fn key_owner_of(owner_user_id: Option<&str>, owner_active: Option<bool>, owner_role: Option<&str>) -> KeyOwner {
    match owner_user_id {
        None => KeyOwner::None,
        Some(_) => match (owner_active, owner_role.and_then(Role::parse)) {
            (Some(true), Some(role)) => KeyOwner::Member(role),
            _ => KeyOwner::Gone,
        },
    }
}

#[cfg(test)]
mod key_owner_tests {
    use super::*;

    /// Payloads review #7 (the parity test's unparseable role): the joined
    /// statement hands back the member's role as stored text, and a text that
    /// names no role grants none: the key's owner counts as gone, as
    /// `get_membership` (which parses the same text with `Role::parse`) makes
    /// the three lookups judge it. Both schemas `CHECK` the column against
    /// the four roles, so no database row can carry such a text and the
    /// parity test, which goes through the database, can't hold this case.
    #[test]
    fn a_role_text_that_names_no_role_grants_none() {
        for text in ["superuser", "Admin", " admin", ""] {
            assert_eq!(key_owner_of(Some("u"), Some(true), Some(text)), KeyOwner::Gone, "{text:?}");
        }
        assert_eq!(key_owner_of(Some("u"), Some(true), Some("admin")), KeyOwner::Member(Role::Admin));
        assert_eq!(key_owner_of(Some("u"), Some(false), Some("admin")), KeyOwner::Gone, "an inactive maker");
        assert_eq!(key_owner_of(Some("u"), None, None), KeyOwner::Gone, "a maker with no row");
        assert_eq!(key_owner_of(None, None, None), KeyOwner::None);
    }
}

/// Optional filters for a tenant-scoped request-log query. Any `None` field is
/// ignored. `from`/`to` bound the `created_at` range (inclusive).
#[derive(Default)]
pub struct RequestLogFilter<'a> {
    pub model: Option<&'a str>,
    pub provider: Option<&'a str>,
    pub status: Option<&'a str>,
    pub from: Option<chrono::DateTime<chrono::Utc>>,
    pub to: Option<chrono::DateTime<chrono::Utc>>,
    /// `(key, value)` pairs the request's `metadata` must all hold (L17).
    pub metadata: &'a [(String, String)],
}

/// Optional filters for a tenant-scoped audit-log query. Any `None` field is
/// ignored. `from`/`to` bound the `created_at` range (inclusive). Named to avoid
/// colliding with the older global `types::AuditLogFilter`.
#[derive(Default)]
pub struct TenantAuditFilter<'a> {
    pub actor_key_id: Option<&'a str>,
    pub action: Option<&'a str>,
    pub from: Option<chrono::DateTime<chrono::Utc>>,
    pub to: Option<chrono::DateTime<chrono::Utc>>,
}

/// Whose entries an audit-log listing reads
/// ([`DatabaseStore::list_audit_log_for_tenant_filtered`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditLogOrg<'a> {
    /// The entries filed under this org: what its members read.
    Org(&'a str),
    /// The entries filed under no org (`org_id` NULL): in hosted mode, the
    /// master key's changes to the gateway and to accounts, which only the
    /// master key reads (`api::audit`).
    NoOrg,
}

/// What [`DatabaseStore::change_own_password`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasswordChange {
    /// Written, and the other sign-ins ended.
    Done,
    /// Nothing written: since the current password was checked, the account's
    /// sign-in ended (a reset, a deactivation or another password change
    /// committed, or the sending session is gone), or its password moved.
    SignInEnded,
}

/// The sign-in a person's request acts under, for a write that must land
/// only while it is live (review round 6, finding 2): a reset, a
/// deactivation or a password change that commits while the request runs
/// ends it, and the write then stores nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignIn<'a> {
    /// No sign-in that any of those ends: the master key (and dev mode,
    /// which acts as it). The write is unconditional.
    MasterKey,
    /// A dashboard session, by its stored id (`sessions.id`, the hash of its
    /// cookie): live while it is at its account's sign-in epoch and the
    /// account is active.
    Session(&'a str),
    /// An app's OAuth grant (`mcp_oauth_grants.id`): live while it is not
    /// revoked, is at its person's sign-in epoch, and the person is active.
    Grant(&'a str),
}

/// What [`DatabaseStore::confirm_password_reset`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResetConfirm {
    /// The token was consumed and `user_id`'s password set; `reactivated`
    /// when the account was deactivated and is active again;
    /// `revoked_invitations`, the pending invitations it had sent (review
    /// round 5, finding 3), for the audit log.
    Done { user_id: String, reactivated: bool, revoked_invitations: Vec<Invitation> },
    /// No unused token by that hash: unknown, already used, or used meanwhile.
    Unusable,
    /// The link's issuer may no longer issue it (inactive, or no longer
    /// outranking the account everywhere, or no longer allowed to reactivate
    /// it): nothing changed (review round 4, findings 4 and 5).
    IssuerRevoked,
}

/// Who owns an org [`DatabaseStore::create_org_with_owner`] makes: an
/// account that exists (`POST /v1/orgs`), or one made with it (a hosted
/// signup, by password or OAuth).
#[derive(Clone, Copy, Debug)]
pub enum OrgOwner<'a> {
    Existing(&'a str),
    New(&'a User),
}

impl OrgOwner<'_> {
    /// The owner's account id.
    pub fn user_id(&self) -> &str {
        match self {
            OrgOwner::Existing(id) => id,
            OrgOwner::New(user) => &user.id,
        }
    }
}

/// Outcome of a guarded write through
/// [`DatabaseStore::update_generation_if_active`].
///
/// The three cases are NOT interchangeable: two of them mean "nothing was
/// written", but only one of them means somebody else is going to finish the
/// job. Callers use this to decide whether they still own the generation's
/// request-log update and its terminal webhook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationWrite {
    /// The row was in flight and this caller wrote it. This caller owns the
    /// request-log update and the terminal webhook.
    Written,
    /// The row had already left `pending`/`processing`: another observer
    /// decided this generation, and IT owns the log update and the webhook.
    /// Writing either here would overturn a decision or double-deliver.
    AlreadyTerminal,
    /// No such row yet. The 3D/video submit handler inserts the generation row
    /// from a spawned task, so a fast first poll can legitimately outrun it.
    /// Nothing was written and there is no competing observer — the caller
    /// still owns the request log (which IS already there, written `pending`
    /// just before), and the poller will drive the row once it appears.
    Missing,
}

impl GenerationWrite {
    /// Whether the caller still owns this generation's terminal side effects
    /// (its request-log update and its webhook). True for everything except
    /// losing the race to another observer.
    pub fn owns_outcome(self) -> bool {
        !matches!(self, GenerationWrite::AlreadyTerminal)
    }
}

/// The one rule every write to a generation's `metadata` follows: a SHALLOW
/// merge. Each top-level key in `patch` replaces the existing key whole (a
/// nested object is replaced, not merged); a `null` deletes the key; keys the
/// patch does not name are kept.
///
/// The row carries the request, its source lineage, stage claims, stage
/// context, the promised formats and the delivered assets, each written by a
/// different step. Replacing the whole column, as writes used to, let each
/// step erase what the others had recorded.
pub fn merge_metadata(existing: Option<&serde_json::Value>, patch: &serde_json::Value) -> serde_json::Value {
    let mut out = match existing {
        Some(serde_json::Value::Object(m)) => m.clone(),
        _ => serde_json::Map::new(),
    };
    if let serde_json::Value::Object(p) = patch {
        for (k, v) in p {
            if v.is_null() {
                out.remove(k);
            } else {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    serde_json::Value::Object(out)
}

/// What `claim_generation_stage` records under `metadata.stage_claims.<stage>`,
/// and `lease_generation_work` under `metadata.leases.<work>`. `at` is the
/// database's clock ([`DatabaseStore::clock_now`]), never a replica's own:
/// every replica judges the record on that one clock.
pub fn stage_claim_record(claimant: &str, at: chrono::DateTime<chrono::Utc>) -> serde_json::Value {
    serde_json::json!({ "by": claimant, "at": at.to_rfc3339() })
}

/// Whether `claimant` may take the lease on `work` that `meta` records: none is
/// recorded, it is `claimant`'s own, or it is at least `ttl` old at `now`, the
/// database's clock (its holder died or stalled). A record that cannot be
/// read counts as lapsed.
pub fn lease_is_free(meta: Option<&serde_json::Value>, work: &str, claimant: &str, ttl: chrono::Duration, now: chrono::DateTime<chrono::Utc>) -> bool {
    let Some(lease) = meta.and_then(|m| m.get("leases")).and_then(|l| l.get(work)) else {
        return true;
    };
    if lease.get("by").and_then(|b| b.as_str()) == Some(claimant) {
        return true;
    }
    match lease.get("at").and_then(|a| a.as_str()).and_then(|a| chrono::DateTime::parse_from_rfc3339(a).ok()) {
        Some(at) => now.signed_duration_since(at) >= ttl,
        None => true,
    }
}

/// `meta`'s `leases` with `work` taken by `claimant` at `now`, the database's
/// clock (`Some`), or released (`None`), as the patch to merge:
/// `{"leases": …}`, or `{"leases": null}` when no lease is left.
pub fn leases_patch(meta: Option<&serde_json::Value>, work: &str, claimant: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> serde_json::Value {
    let mut leases = meta
        .and_then(|m| m.get("leases"))
        .filter(|l| l.is_object())
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let map = leases.as_object_mut().expect("an object");
    match claimant {
        Some(c) => {
            map.insert(work.to_string(), stage_claim_record(c, now));
        }
        None => {
            map.remove(work);
        }
    }
    if map.is_empty() {
        serde_json::json!({ "leases": null })
    } else {
        serde_json::json!({ "leases": leases })
    }
}

/// A delivery of the durable webhook outbox (migration 029), as a dispatcher
/// claims it (`proxy::webhook::Outbox`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxDelivery {
    /// The delivery id, `whd-` and a UUID: `X-Litegen-Delivery-Id` on every
    /// attempt of this delivery.
    pub id: String,
    pub generation_id: String,
    pub key_id: String,
    pub org_id: Option<String>,
    /// The attempts sent before this claim.
    pub attempts: i32,
    /// Whether a redeliver made it due again (`redelivered_at` is set): its
    /// series was asked for, so it is sent even when an earlier attempt for
    /// its job was answered 2xx.
    pub redelivered: bool,
}

/// A delivery of the outbox as stored: where it stands and when its next
/// attempt is due. Times are the database's.
#[derive(Debug, Clone, PartialEq)]
pub struct OutboxRecord {
    pub id: String,
    pub generation_id: String,
    pub key_id: String,
    pub org_id: Option<String>,
    pub app_id: Option<String>,
    /// `pending`, `delivered` or `failed`.
    pub status: String,
    /// The attempts sent so far.
    pub attempts: i32,
    pub next_attempt_at: chrono::DateTime<chrono::Utc>,
    pub claimed_by: Option<String>,
    pub claimed_until: Option<chrono::DateTime<chrono::Utc>>,
    pub last_status_code: Option<i32>,
    pub last_error: Option<String>,
    /// The payload of the latest attempt: stored once per delivery, not on
    /// each attempt's row.
    pub payload_json: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    /// When a redeliver last made it due again (migration 032).
    pub redelivered_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// What one attempt of an outbox delivery decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxOutcome {
    /// Answered 2xx: the delivery is done.
    Delivered,
    /// Failed: the next attempt is due `after` from now, on the database's clock.
    Retry { after: chrono::Duration },
    /// Done without a 2xx: answered 410 Gone, its last attempt failed, or it
    /// can't be sent (the key or its webhook_url is gone, the URL is refused).
    Failed,
}

impl OutboxOutcome {
    /// The delivery's status after this outcome: `pending` while another
    /// attempt is to come.
    pub fn status(self) -> &'static str {
        match self {
            OutboxOutcome::Delivered => "delivered",
            OutboxOutcome::Retry { .. } => "pending",
            OutboxOutcome::Failed => "failed",
        }
    }

    /// How long from now the next attempt is due: for a retry only.
    pub fn retry_after(self) -> Option<chrono::Duration> {
        match self {
            OutboxOutcome::Retry { after } => Some(after),
            _ => None,
        }
    }
}

/// An attempt of an outbox delivery, as [`DatabaseStore::finish_webhook_attempt`]
/// records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxAttempt<'a> {
    /// The attempts sent, this one included (unchanged when nothing was sent).
    pub attempts: i32,
    pub outcome: OutboxOutcome,
    pub status_code: Option<i32>,
    pub error: Option<&'a str>,
    /// The payload this attempt sent; `None` keeps the stored one.
    pub payload_json: Option<&'a str>,
}

/// What asking for a delivery to be sent again found
/// ([`DatabaseStore::redeliver_webhook_delivery`]).
#[derive(Debug, Clone, PartialEq)]
pub enum Redelivery {
    /// It had ended (`delivered` or `failed`): it is due again now, a fresh
    /// series of attempts under the same id. Its state now.
    Redelivered(OutboxRecord),
    /// It is still being sent: nothing changed.
    Pending(OutboxRecord),
    /// It was redelivered less than the cooldown ago: nothing changed. It
    /// may be asked for again at `retry_at`, on the database's clock.
    TooSoon { record: OutboxRecord, retry_at: chrono::DateTime<chrono::Utc> },
    /// The key has no delivery with that id.
    NotFound,
}

/// What a redeliver that changed nothing found, from `record` as stored now
/// (`None`: the key has no such delivery): still being sent, or ended and
/// redelivered less than `cooldown` ago.
pub(crate) fn redelivery_refused(record: Option<OutboxRecord>, cooldown: chrono::Duration) -> Redelivery {
    match record {
        None => Redelivery::NotFound,
        Some(record) if record.status == "pending" => Redelivery::Pending(record),
        Some(record) => {
            let retry_at = match record.redelivered_at {
                Some(at) => at + cooldown,
                // Ended and never redelivered: a race with another change; ask again at once.
                None => record.finished_at.unwrap_or(record.next_attempt_at),
            };
            Redelivery::TooSoon { record, retry_at }
        }
    }
}

/// What one purge of the webhook log deleted
/// ([`DatabaseStore::purge_webhook_log`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WebhookLogPurged {
    /// Outbox deliveries that had ended.
    pub deliveries: u64,
    /// Attempt rows (`webhook_deliveries`).
    pub attempts: u64,
}

/// A file of a deleted org or app, waiting to be deleted from litegen's own
/// store (migration 031), as a worker claims it (`proxy::tenant_files`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantFileDeletion {
    pub id: i64,
    /// What was deleted: `org` or `app`.
    pub scope: String,
    /// The deleted org's or app's id.
    pub tenant_id: String,
    pub generation_id: String,
    /// The file's URL, as its generation recorded it.
    pub url: String,
    /// The tries so far, this claim included.
    pub attempts: i32,
}

/// The URLs of the files litegen kept for a generation, from its stored
/// `result_url` and `metadata`: each recorded asset's (`metadata.assets`)
/// that isn't `kept: false`; for a row that records no assets (from before
/// litegen kept results, or whose metadata isn't JSON), its one `result_url`,
/// unless it kept nothing (`keep_outputs: false`). What deleting its org or
/// app must delete (migration 031). Each URL once.
pub(crate) fn kept_file_urls(result_url: Option<&str>, metadata: Option<&str>) -> Vec<String> {
    let meta: Option<serde_json::Value> = metadata.and_then(|m| serde_json::from_str(m).ok());
    let assets = meta.as_ref().and_then(|m| m.get("assets")).and_then(|a| a.as_array()).filter(|a| !a.is_empty());
    let mut urls: Vec<String> = Vec::new();
    match assets {
        Some(assets) => {
            for asset in assets {
                let kept = asset.get("kept").and_then(|k| k.as_bool()).unwrap_or(true);
                let url = asset.get("url").and_then(|u| u.as_str()).filter(|u| kept && !u.is_empty());
                if let Some(url) = url.filter(|url| !urls.iter().any(|u| u == url)) {
                    urls.push(url.to_string());
                }
            }
        }
        None => {
            let kept_nothing = meta.as_ref().and_then(|m| m.get("keep_outputs")).and_then(|k| k.as_bool()) == Some(false);
            if let Some(url) = result_url.filter(|u| !kept_nothing && !u.is_empty()) {
                urls.push(url.to_string());
            }
        }
    }
    urls
}

/// How long an `Idempotency-Key` record answers for its request.
pub const IDEMPOTENCY_TTL: chrono::Duration = chrono::Duration::hours(24);

/// How long a record may stay `in_progress` with no lease: one an older
/// replica made during a rolling upgrade, or one left from before migration
/// 030 (this version always writes a lease). A record still running past it
/// belongs to a process that died, and the key is free again. A leased record
/// ([`DatabaseStore::idempotency_begin_held`]) is free once its lease lapses
/// instead, and held for as long as its request renews it.
pub const IDEMPOTENCY_IN_PROGRESS_STALE: chrono::Duration = chrono::Duration::minutes(15);

/// A response stored under an `Idempotency-Key`, replayed byte for byte.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// What claiming an `Idempotency-Key` found.
#[derive(Debug, Clone, PartialEq)]
pub enum IdempotencyBegin {
    /// No live record: this request runs, and records its response.
    Started,
    /// The same request already finished: replay this.
    Replay(StoredResponse),
    /// The key was used for a different request.
    Conflict,
    /// The same request is still running.
    InProgress,
}

/// Trait defining the database operations. Implementations exist for SQLite and PostgreSQL.
#[async_trait]
#[allow(clippy::too_many_arguments)] // DB trait methods need many params; would need wrapper structs to reduce
pub trait DatabaseStore: Send + Sync {
    // ─── Generations ────────────────────────────────────────────────────────────

    /// Insert a new generation row when a video generation is submitted.
    /// `org_id`/`app_id` stamp the tenant; pass `None` when there is no tenant context.
    async fn insert_generation(
        &self,
        id: &str,
        key_id: Option<&Uuid>,
        model: &str,
        provider: &str,
        media_type: &str,
        provider_job_id: Option<&str>,
        cost_usd: f64,
        org_id: Option<&str>,
        app_id: Option<&str>,
    ) -> Result<(), sqlx::Error>;

    /// Update the status (and related fields) of an existing generation.
    async fn update_generation_status(
        &self,
        id: &str,
        status: &str,
        progress: i32,
        result_url: Option<&str>,
        error: Option<&str>,
        completed_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<(), sqlx::Error>;

    /// Replace a generation's `metadata` JSON blob. Used by the 3D path to
    /// persist the re-hosted asset list; `metadata` is already a nullable TEXT
    /// column in both backends, so this needs no migration.
    async fn update_generation_metadata(
        &self,
        id: &str,
        metadata: &serde_json::Value,
    ) -> Result<(), sqlx::Error>;

    /// Advance a generation that is still IN FLIGHT, in one guarded UPDATE.
    /// Returns which of the three [`GenerationWrite`] cases happened — none of
    /// them is an error:
    ///
    /// - [`GenerationWrite::Written`]: the row was `pending`/`processing` and
    ///   this caller wrote it, so it owns the request-log update and the
    ///   terminal webhook.
    /// - [`GenerationWrite::AlreadyTerminal`]: another observer decided this
    ///   generation first. Nothing was written, and the caller must skip BOTH
    ///   the log update and the webhook — they belong to the winner.
    /// - [`GenerationWrite::Missing`]: the row does not exist yet, because the
    ///   async submit handler inserts it from a spawned task a fast first poll
    ///   can outrun. Nothing was written and there is no competing observer, so
    ///   the caller keeps its request-log update (that row IS already there,
    ///   written `pending` at submit) and skips only the webhook; the poller
    ///   drives the generation once the insert lands.
    ///
    /// [`GenerationWrite::owns_outcome`] collapses the first and third cases
    /// for callers that only need "may I still touch the request log?".
    ///
    /// Two things the split `update_generation_status` +
    /// `update_generation_metadata` pair cannot give the async families:
    ///
    /// 1. **Guarded.** A 3D generation has TWO terminal observers — the
    ///    background poller and `GET /v1/models3d/{id}`, which the SDK polls
    ///    more often than the poller ticks. An unguarded write lets the loser of
    ///    that race overwrite a decided outcome: a cancel resurrected as
    ///    `completed`, a completed row reaped as `failed`, or a stale
    ///    `processing` tick reverting a finished row. `WHERE status IN
    ///    ('pending','processing')` makes the first observer the only one, and
    ///    the returned flag tells the loser to skip its request-log update and
    ///    its webhook so neither fires twice.
    /// 2. **Atomic.** `status`, `result_url` and `metadata` move together, so a
    ///    row can never be left permanently `completed` with no mesh in its
    ///    metadata — the state Ruling R21 forbids (the consumer refunds while
    ///    litegen billed).
    ///
    /// `completed_at` is `None` for a mid-flight progress update and `Some(now)`
    /// for a terminal one. `metadata` is `None` to leave the column untouched,
    /// and otherwise a patch MERGED into it ([`merge_metadata`]), never a
    /// replacement: what submit recorded survives the terminal write.
    ///
    /// The default implementation is the unguarded, non-atomic pair — correct
    /// enough for in-memory test doubles, never used by a real backend: both
    /// SQLite and Postgres override it.
    async fn update_generation_if_active(
        &self,
        id: &str,
        status: &str,
        progress: i32,
        result_url: Option<&str>,
        error: Option<&str>,
        completed_at: Option<chrono::DateTime<chrono::Utc>>,
        metadata: Option<&serde_json::Value>,
    ) -> Result<GenerationWrite, sqlx::Error> {
        let existing = self.get_generation(id).await?.and_then(|g| g.metadata);
        self.update_generation_status(id, status, progress, result_url, error, completed_at).await?;
        if let Some(patch) = metadata {
            self.update_generation_metadata(id, &merge_metadata(existing.as_ref(), patch)).await?;
        }
        Ok(GenerationWrite::Written)
    }

    /// Merge `patch` into a generation's `metadata` (see [`merge_metadata`])
    /// ONLY while the row is still in flight.
    ///
    /// Guarded because a fast client can poll a job to completion between the
    /// submit path's row insert and this write; landing second, it must not
    /// touch a decided row. Atomic in the real backends: a compare-and-swap on
    /// the column, retried on contention, so two writers merging different
    /// keys at once both land.
    ///
    /// The default implementation reads then writes, correct enough for
    /// in-memory test doubles; SQLite and Postgres both override it.
    async fn merge_generation_metadata_if_active(
        &self,
        id: &str,
        patch: &serde_json::Value,
    ) -> Result<GenerationWrite, sqlx::Error> {
        match self.get_generation(id).await? {
            None => Ok(GenerationWrite::Missing),
            Some(g) if g.status.is_terminal() => Ok(GenerationWrite::AlreadyTerminal),
            Some(g) => {
                self.update_generation_metadata(id, &merge_metadata(g.metadata.as_ref(), patch)).await?;
                Ok(GenerationWrite::Written)
            }
        }
    }

    /// Claim the right to start a billed `stage` of an in-flight generation.
    ///
    /// `true` for exactly one caller per (generation, stage): it records
    /// `metadata.stage_claims.<stage> = {by, at}` only if no claim is there
    /// and the row is still `pending`/`processing`. Both observers of a job
    /// (the client's poll and the background poller) can see a stage finish;
    /// the claim is what stops them both from starting the next billed one.
    ///
    /// The default implementation is not atomic (test doubles only); SQLite
    /// and Postgres override it with a compare-and-swap.
    async fn claim_generation_stage(&self, id: &str, stage: &str, claimant: &str) -> Result<bool, sqlx::Error> {
        let Some(g) = self.get_generation(id).await? else { return Ok(false) };
        if g.status.is_terminal()
            || g.metadata.as_ref().and_then(|m| m.get("stage_claims")).and_then(|c| c.get(stage)).is_some()
        {
            return Ok(false);
        }
        let mut claims = g.metadata.as_ref().and_then(|m| m.get("stage_claims")).cloned().unwrap_or_else(|| serde_json::json!({}));
        claims[stage] = stage_claim_record(claimant, self.clock_now().await?);
        self.update_generation_metadata(id, &merge_metadata(g.metadata.as_ref(), &serde_json::json!({ "stage_claims": claims }))).await?;
        Ok(true)
    }

    /// Lease the work named `work` on an in-flight generation for `ttl`: the
    /// way one observer, of the several that can see the same job finish,
    /// does its expensive follow-up (a 3D delivery's conversion and re-host)
    /// while the others stand aside.
    ///
    /// `true` when [`lease_is_free`] says `claimant` may take it; the grant
    /// records `metadata.leases.<work> = {by, at}`, so of racing claimants
    /// exactly one wins. `false` while another claimant's lease is live, and
    /// for a row no longer in flight, whose work is done. A generation with no
    /// row yet is granted without a write: the submit handler spawns its
    /// insert, a fast poll can outrun it, and no poller works on a row it
    /// cannot list.
    ///
    /// The default implementation is not atomic (test doubles only); SQLite
    /// and Postgres override it with a compare-and-swap.
    async fn lease_generation_work(&self, id: &str, work: &str, claimant: &str, ttl: chrono::Duration) -> Result<bool, sqlx::Error> {
        let Some(g) = self.get_generation(id).await? else { return Ok(true) };
        let now = self.clock_now().await?;
        if g.status.is_terminal() || !lease_is_free(g.metadata.as_ref(), work, claimant, ttl, now) {
            return Ok(false);
        }
        let patch = leases_patch(g.metadata.as_ref(), work, Some(claimant), now);
        self.update_generation_metadata(id, &merge_metadata(g.metadata.as_ref(), &patch)).await?;
        Ok(true)
    }

    /// Release `claimant`'s lease on `work`, so the next observer need not
    /// wait for it to lapse: for work that ended without settling the row. A
    /// lease held by someone else, or on a row no longer in flight, is left.
    async fn release_generation_work(&self, id: &str, work: &str, claimant: &str) -> Result<(), sqlx::Error> {
        let Some(g) = self.get_generation(id).await? else { return Ok(()) };
        let held = g.metadata.as_ref().and_then(|m| m.get("leases")).and_then(|l| l.get(work)).and_then(|l| l.get("by")).and_then(|b| b.as_str());
        if g.status.is_terminal() || held != Some(claimant) {
            return Ok(());
        }
        let patch = leases_patch(g.metadata.as_ref(), work, None, self.clock_now().await?);
        self.update_generation_metadata(id, &merge_metadata(g.metadata.as_ref(), &patch)).await
    }

    // ─── Idempotency keys ───────────────────────────────────────────────

    /// Claim `key` for the request hashing to `request_hash`, within `scope`
    /// (the owner: one org, app and key), the record held by `held_by` for
    /// `lease` from the database's now, which the request renews while it
    /// runs (migration 030). The first caller gets `Started` and then calls
    /// [`idempotency_complete_held`](Self::idempotency_complete_held) or
    /// [`idempotency_release_held`](Self::idempotency_release_held). Records
    /// older than [`IDEMPOTENCY_TTL`] are forgotten, and so is a record in
    /// progress whose lease lapsed, or, with no lease (an older replica's),
    /// one older than [`IDEMPOTENCY_IN_PROGRESS_STALE`]. The default (test
    /// doubles) keeps nothing.
    async fn idempotency_begin_held(
        &self,
        _scope: &str,
        _key: &str,
        _request_hash: &str,
        _held_by: &str,
        _lease: chrono::Duration,
    ) -> Result<IdempotencyBegin, sqlx::Error> {
        Ok(IdempotencyBegin::Started)
    }

    /// Store the response of the request that holds `key` as `held_by`, only
    /// while it still holds it: once its lease lapsed and a retry took the
    /// key over, the first request's response is not stored over the retry's
    /// record. The default (test doubles) keeps nothing.
    async fn idempotency_complete_held(&self, _scope: &str, _key: &str, _held_by: &str, _response: &StoredResponse) -> Result<(), sqlx::Error> {
        Ok(())
    }

    /// Forget `key`, which its request (`held_by`) was refused before it ran
    /// anything, only while that request still holds it, so a retry's record
    /// is not deleted. The default (test doubles) keeps nothing.
    async fn idempotency_release_held(&self, _scope: &str, _key: &str, _held_by: &str) -> Result<(), sqlx::Error> {
        Ok(())
    }

    /// Fetch a generation by its local ID.
    async fn get_generation(&self, id: &str) -> Result<Option<Generation>, sqlx::Error>;

    /// List pending/processing generations for the background poller (capped at `limit`).
    async fn list_active_generations(&self, limit: u32) -> Result<Vec<Generation>, sqlx::Error>;

    /// Claim up to `limit` pending/processing generations for `claimant`'s
    /// poller, least recently polled first (a row never polled counts from its
    /// creation), and return them. A row is free when nobody holds it, its hold
    /// has run out on the database's clock, or `claimant` holds it already (a
    /// replica re-takes its own rows while it has room for them; with more
    /// than `limit`, its claims take them in turn). Claiming sets
    /// the holder to `claimant` and the hold to `hold` from the database's now,
    /// so a dead replica's rows pass to another one once its last claim lapses.
    /// Postgres skips rows another claimer has locked (`FOR UPDATE SKIP
    /// LOCKED`); SQLite serializes its writers.
    ///
    /// `hold` should be positive and modest (the poller uses 30 s): a hold
    /// under a millisecond lapses at once, and a huge one can overflow SQLite's
    /// date range.
    ///
    /// The default makes no claims: it lists every active row, which is all a
    /// test double with a single poller needs. A production store must
    /// override it, or every replica polls every row.
    async fn claim_active_generations(
        &self,
        limit: u32,
        _claimant: &str,
        _hold: chrono::Duration,
    ) -> Result<Vec<Generation>, sqlx::Error> {
        self.list_active_generations(limit).await
    }

    /// Give back every poll claim `claimant` holds: its rows have no holder
    /// from now on, so another replica's next claim takes them at once, not
    /// after the hold lapses. Returns how many rows it freed. A replica calls
    /// this when its poller has stopped for good (a graceful shutdown); a
    /// replica that dies leaves its claims to lapse instead.
    ///
    /// The default has no claims to free and returns 0, like the default of
    /// [`Self::claim_active_generations`]; a store that overrides that must
    /// override this.
    async fn release_poll_claims(&self, _claimant: &str) -> Result<u64, sqlx::Error> {
        Ok(0)
    }

    /// Extend `claimant`'s poll claims on those of `ids` it still holds and
    /// that are still active, to `hold` from the database's now. Returns how
    /// many it extended: a row another replica took after this claim lapsed,
    /// or one that has ended, is not counted. The poller calls it for the rows
    /// a long tick is still polling, and before polling a row that waited.
    ///
    /// The default makes no claims, like [`Self::claim_active_generations`]'s,
    /// so it counts every row as still held.
    async fn renew_poll_claims(&self, _claimant: &str, ids: &[String], _hold: chrono::Duration) -> Result<u64, sqlx::Error> {
        Ok(ids.len() as u64)
    }

    /// Record that the poller warns, at 10-minute `mark` of row `id`'s age (0
    /// from its creation, 1 from ten minutes on, and so on), that it skips the
    /// row for want of a key of the vendor account its job ran on. `true` when
    /// no warning at this mark or a later one was recorded yet, so this caller
    /// warns; `false` when one was (an earlier poll at this mark, on any
    /// replica, warned). Only an in-flight row is marked
    /// (`poll_skip_warned_mark`, migration 035), so each mark warns once
    /// however the polls fall.
    ///
    /// The default records nothing and says `true`: a test double warns at
    /// every skip.
    async fn note_poll_skip_warning(&self, _id: &str, _mark: i64) -> Result<bool, sqlx::Error> {
        Ok(true)
    }

    /// The database's clock: the one time every replica shares, so leases,
    /// stage claims, idempotency records and the poller's ages are stamped
    /// and judged on it, never on a replica's own clock (which may be off).
    ///
    /// The default is this process's clock, which is right for a store in
    /// this process (the test doubles).
    async fn clock_now(&self) -> Result<chrono::DateTime<chrono::Utc>, sqlx::Error> {
        Ok(chrono::Utc::now())
    }

    /// Paginated list of generations.
    /// When `key_id` is `Some`, returns rows owned by that key OR rows with `key_id IS NULL`
    /// (master-key rows). When `key_id` is `None` (caller is master key), returns all rows.
    // superseded by list_generations_for_tenant for the API (kept for tests/back-compat)
    async fn list_generations(
        &self,
        key_id: Option<&Uuid>,
        page: u32,
        per_page: u32,
    ) -> Result<Vec<Generation>, sqlx::Error>;

    /// Count of generations, with the same ownership filter as `list_generations`.
    // superseded by count_generations_for_tenant for the API (kept for tests/back-compat)
    async fn count_generations(&self, key_id: Option<&Uuid>) -> Result<i64, sqlx::Error>;

    /// Soft-cancel a generation: sets `status = 'cancelled'` and `completed_at = NOW()`.
    /// Only rows with `status IN ('pending', 'processing')` are affected.
    /// Returns the updated row, or `None` if the id doesn't exist or isn't cancellable.
    async fn cancel_generation(&self, id: &str) -> Result<Option<Generation>, sqlx::Error>;

    // ─── Request Logs ───────────────────────────────────────────────────

    /// `org_id`/`app_id` stamp the tenant; pass `None` when there is no tenant context.
    async fn log_request(
        &self,
        id: &str,
        model: &str,
        provider: &str,
        status: &str,
        media_type: &str,
        cost_usd: f64,
        latency_ms: i64,
        error: Option<&str>,
        metadata: Option<&serde_json::Value>,
        org_id: Option<&str>,
        app_id: Option<&str>,
    ) -> Result<(), sqlx::Error>;

    /// Move an async (`video`/`model3d`) request's log row — written `pending`
    /// at submit, under the generation's id — to its terminal status:
    /// `completed`, `failed` or `cancelled`, the vocabulary the log reader
    /// parses, with the error when it failed. Only `status` and `error` change.
    /// Called on every terminal transition; callers log a failure and carry on,
    /// since a stale log row must never fail a request or a poller tick.
    async fn update_request_log_status(
        &self,
        id: &str,
        status: &str,
        error: Option<&str>,
    ) -> Result<(), sqlx::Error>;

    /// Claim the refund of a `failed` or `cancelled` generation billed
    /// `cost_usd` (L18): zero its cost, and its request log's, if the row
    /// still holds exactly that charge. True for exactly one caller, however
    /// many observers of the outcome ask, so it is refunded once.
    async fn claim_generation_refund(&self, _id: &str, _cost_usd: f64) -> Result<bool, sqlx::Error> {
        Ok(false)
    }

    // superseded by get_request_logs_for_tenant for the API (kept for tests/back-compat)
    async fn get_request_logs(
        &self,
        page: u32,
        per_page: u32,
    ) -> Result<(Vec<RequestLog>, u64), sqlx::Error>;

    /// Filtered, paginated request logs. Any `None` filter field is ignored.
    // superseded by get_request_logs_for_tenant for the API (kept for tests/back-compat)
    async fn get_request_logs_filtered(
        &self,
        filters: &LogFilters,
        page: u32,
        per_page: u32,
    ) -> Result<(Vec<RequestLog>, u64), sqlx::Error>;

    // ─── API Keys ───────────────────────────────────────────────────────

    async fn create_api_key(
        &self,
        name: &str,
        key_hash: &str,
        key_prefix: &str,
        token_quota: Option<f64>,
        rpm_limit: Option<u32>,
        scopes: &str,
        webhook_url: Option<&str>,
    ) -> Result<ApiKey, sqlx::Error>;

    async fn get_api_key(&self, id: &Uuid) -> Result<Option<ApiKey>, sqlx::Error>;

    async fn update_api_key(
        &self,
        id: &Uuid,
        req: &UpdateApiKeyRequest,
    ) -> Result<Option<ApiKey>, sqlx::Error>;

    /// Look up a key by its SHA-256 hash. Only returns active, non-expired keys.
    async fn lookup_api_key_by_hash(&self, key_hash: &str) -> Result<Option<ApiKey>, sqlx::Error>;

    /// The active key with hash `key_hash`, and where the person who made it
    /// stands in the key's org ([`KeyOwner`]): what authenticating a key
    /// needs, in one statement where the store can (review 2026-11-11, P14).
    /// A key with no org counts as `null_org`'s (`None`: no org at all, so an
    /// owned key with no org is [`KeyOwner::Gone`]). This default runs the
    /// three lookups it replaces.
    async fn lookup_api_key_with_owner(&self, key_hash: &str, null_org: Option<&str>) -> Result<Option<(ApiKey, KeyOwner)>, sqlx::Error> {
        key_owner_by_lookups(self, key_hash, null_org).await
    }

    /// Atomically add `cost_usd` to `tokens_used`.
    /// Returns the new `tokens_used` value.
    /// Returns `sqlx::Error::RowNotFound` if the key doesn't exist.
    async fn atomic_charge_tokens(
        &self,
        id: &Uuid,
        cost_usd: f64,
    ) -> Result<f64, sqlx::Error>;

    /// Atomically reserve `amount` of quota against a key BEFORE dispatching a
    /// (billable) generation. Adds `amount` to `tokens_used` in a single
    /// conditional statement, but only if the key has no quota or the new total
    /// would stay within `token_quota`. Returns `true` if the reservation
    /// succeeded, `false` if it would exceed the quota (or the key is missing).
    ///
    /// This is what actually enforces the quota: because the check-and-increment
    /// is one atomic UPDATE, concurrent requests cannot all pass a stale
    /// pre-flight read, and a single oversized request is rejected up front.
    /// The caller reconciles the delta (or releases the hold) after the
    /// generation completes via [`atomic_charge_tokens`].
    async fn reserve_tokens(
        &self,
        _id: &Uuid,
        _amount: f64,
    ) -> Result<bool, sqlx::Error> {
        // Default is permissive so lightweight test doubles need not implement
        // quota logic. The production SqliteDatabase/PostgresDatabase impls
        // override this with the atomic conditional reservation.
        Ok(true)
    }

    async fn validate_api_key(&self, key_hash: &str) -> Result<Option<ApiKey>, sqlx::Error>;

    async fn list_api_keys(&self) -> Result<Vec<ApiKey>, sqlx::Error>;

    /// List API keys owned by a specific user.
    async fn list_api_keys_for_owner(&self, _owner_user_id: &str) -> Result<Vec<ApiKey>, sqlx::Error> {
        Ok(vec![])
    }

    /// Set the owner_user_id of a key (called after creation for session-authed users).
    async fn set_api_key_owner(&self, _id: &Uuid, _owner_user_id: &str) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn revoke_api_key(&self, id: &Uuid) -> Result<bool, sqlx::Error>;

    /// Rotate an existing key's secret in place: replace `key_hash`/`key_prefix`
    /// (and `public_id`) on the same row id, keeping all other settings.
    /// `owner`, when given, becomes the key's `owner_user_id` in the same
    /// statement: whoever receives the new secret is who the key acts for.
    /// `None` keeps the owner the key had.
    /// Returns the updated key, or `None` if the id doesn't exist.
    /// Default impl returns None (for mock/test implementations).
    async fn rotate_api_key(
        &self,
        _id: &Uuid,
        _public_id: &str,
        _key_hash: &str,
        _key_prefix: &str,
        _owner: Option<&str>,
    ) -> Result<Option<ApiKey>, sqlx::Error> {
        Ok(None)
    }

    // ─── Stats ──────────────────────────────────────────────────────────

    // superseded by get_stats_for_tenant for the API (kept for tests/back-compat)
    async fn get_stats(&self) -> Result<ProxyStats, sqlx::Error>;

    /// Tenant-scoped aggregate stats: same shape as `get_stats` but restricted to
    /// `org_id` (and `app_id` when `Some`), and to requests whose `metadata`
    /// holds every `(key, value)` pair given (L17). Default impl returns empty
    /// stats so mock/test implementations still compile.
    async fn get_stats_for_tenant(
        &self,
        _org_id: &str,
        _app_id: Option<&str>,
        _metadata: &[(String, String)],
    ) -> Result<ProxyStats, sqlx::Error> {
        Ok(ProxyStats {
            total_requests: 0,
            successful_requests: 0,
            failed_requests: 0,
            total_cost_usd: 0.0,
            avg_latency_ms: 0.0,
            requests_per_minute: 0.0,
            models_used: Vec::new(),
            providers_used: Vec::new(),
            latency_percentiles: LatencyPercentiles {
                p50_ms: 0.0,
                p95_ms: 0.0,
                p99_ms: 0.0,
                sample_count: 0,
                window_minutes: 60,
            },
        })
    }

    /// Compute latency percentiles (p50/p95/p99) for completed requests in the
    /// last `since_minutes` minutes.  Capped at 10 000 samples.
    /// Default impl returns zeroed percentiles (for mock/test implementations).
    async fn latency_percentiles(&self, since_minutes: i64) -> Result<LatencyPercentiles, sqlx::Error> {
        Ok(LatencyPercentiles { p50_ms: 0.0, p95_ms: 0.0, p99_ms: 0.0, sample_count: 0, window_minutes: since_minutes })
    }

    // ─── Audit Log ──────────────────────────────────────────────────────

    /// Insert a single audit log entry, under the org it names; `org_id` None
    /// files it under no org. The server picks the org (`api::audit::audit_org`).
    /// Default impl is a no-op (for mock/test implementations).
    async fn insert_audit_log(&self, _entry: &AuditLogEntry) -> Result<(), sqlx::Error> {
        Ok(())
    }

    /// Move the audit entries filed under `org_id` that the master key wrote
    /// (no `actor_key_id`, `actor_label` `"master-key"`, as every audit writer
    /// records it) for one of `actions` to no org (`org_id` NULL). Returns
    /// how many moved; a second call moves none. The hosted start-up step
    /// `api::audit::move_operator_entries_out_of_the_default_org`.
    /// Default impl moves nothing (for mock/test implementations).
    async fn move_master_key_audit_entries_to_no_org(&self, _org_id: &str, _actions: &[&str]) -> Result<u64, sqlx::Error> {
        Ok(0)
    }

    /// Paginated list of audit log entries with optional filters.
    /// Returns `(entries, total_count)`.
    /// Default impl returns an empty list (for mock/test implementations).
    async fn list_audit_log(
        &self,
        _filter: &AuditLogFilter,
        _page: u32,
        _per_page: u32,
    ) -> Result<(Vec<AuditLogEntry>, i64), sqlx::Error> {
        Ok((Vec::new(), 0))
    }

    // ─── Webhook Delivery Log ────────────────────────────────────────────

    /// Record one webhook delivery attempt.
    /// Default impl is a no-op (for mock/test implementations).
    async fn insert_webhook_delivery(&self, _delivery: &WebhookDelivery) -> Result<(), sqlx::Error> {
        Ok(())
    }

    /// Paginated list of webhook deliveries for a specific key.
    /// Returns `(deliveries, total_count)`.
    /// Default impl returns an empty list (for mock/test implementations).
    async fn list_webhook_deliveries(
        &self,
        _key_id: &str,
        _page: u32,
        _per_page: u32,
    ) -> Result<(Vec<WebhookDelivery>, i64), sqlx::Error> {
        Ok((Vec::new(), 0))
    }

    // ─── Webhook outbox (migration 029) ─────────────────────────────────

    /// Whether this store keeps the durable webhook outbox: its terminal
    /// writes record each terminal webhook as a pending delivery
    /// (`webhook_outbox`, by the trigger of migration 029), which a
    /// dispatcher claims and sends (`proxy::webhook::Outbox`). The default
    /// keeps none, so a test double's webhooks go out from memory.
    fn keeps_webhook_outbox(&self) -> bool {
        false
    }

    /// The outbox delivery of `generation_id`, as stored; `None` when its
    /// terminal write recorded none.
    async fn webhook_outbox_of(&self, _generation_id: &str) -> Result<Option<OutboxRecord>, sqlx::Error> {
        Ok(None)
    }

    /// Claim the pending delivery of `generation_id` whose first attempt
    /// hasn't been sent, for `claimant`, for `hold` from the database's now,
    /// whatever its due time: the terminal observer's first attempt, sent at
    /// once, within the delivery's first-attempt grace (migration 032) when
    /// no dispatcher's tick takes it. `None` when it has none, its first
    /// attempt has gone (a dispatcher's, after the grace), or another
    /// claimant's lease on it is live.
    async fn claim_webhook_delivery_of(
        &self,
        _generation_id: &str,
        _claimant: &str,
        _hold: chrono::Duration,
    ) -> Result<Option<OutboxDelivery>, sqlx::Error> {
        Ok(None)
    }

    /// Claim up to `limit` pending deliveries that are due and that nobody
    /// holds a live lease on, for `claimant`, for `hold` from the database's
    /// now, the longest due first. Of one org, `claimant` holds at most
    /// `per_org` at once, the ones it holds already counted, so one org's
    /// backlog can't take every claim.
    async fn claim_due_webhook_deliveries(
        &self,
        _claimant: &str,
        _limit: u32,
        _per_org: u32,
        _hold: chrono::Duration,
    ) -> Result<Vec<OutboxDelivery>, sqlx::Error> {
        Ok(Vec::new())
    }

    /// Whether a delivery is due: one pending, whose next attempt is due and
    /// that nobody holds a live lease on, on the database's clock. A read, so
    /// an idle dispatcher's tick writes nothing (on SQLite a claim takes the
    /// write lock). The default says yes, so a store that overrides only the
    /// claim still claims every tick.
    async fn webhook_delivery_due(&self) -> Result<bool, sqlx::Error> {
        Ok(true)
    }

    /// Whether an attempt at `generation_id`'s webhook was answered 2xx, as
    /// `webhook_deliveries` records it, whatever its delivery: a replica from
    /// before the outbox (migration 029) records its attempts with no
    /// delivery id.
    async fn webhook_answered_for(&self, _generation_id: &str) -> Result<bool, sqlx::Error> {
        Ok(false)
    }

    /// Extend `claimant`'s leases on those of `ids` it still holds and that
    /// are still pending, to `hold` from the database's now. Returns how many.
    async fn renew_webhook_claims(&self, _claimant: &str, _ids: &[String], _hold: chrono::Duration) -> Result<u64, sqlx::Error> {
        Ok(0)
    }

    /// Give back every lease `claimant` holds on a pending delivery but
    /// those of `keep` (attempts still sending), so any dispatcher may claim
    /// those at once. Returns how many it freed.
    async fn release_webhook_claims(&self, _claimant: &str, _keep: &[String]) -> Result<u64, sqlx::Error> {
        Ok(0)
    }

    /// Record what an attempt of delivery `id` decided and give its lease
    /// back, only while `claimant` holds it and the delivery is pending.
    /// Returns whether it did: `false` means another claimant took the
    /// delivery after this one's lease lapsed, and its attempt decides.
    async fn finish_webhook_attempt(&self, _id: &str, _claimant: &str, _attempt: &OutboxAttempt<'_>) -> Result<bool, sqlx::Error> {
        Ok(false)
    }

    /// Make key `key_id`'s delivery `delivery_id` due again now, if it has
    /// ended (`delivered` or `failed`) and wasn't redelivered within
    /// `cooldown`, on the database's clock: a fresh series of attempts under
    /// the same id (`attempts` back to 0, so its attempts are numbered from 1
    /// again and it gets the whole retry schedule), its last outcome
    /// cleared, `redelivered_at` now. Anything else changes nothing.
    async fn redeliver_webhook_delivery(&self, _key_id: &str, _delivery_id: &str, _cooldown: chrono::Duration) -> Result<Redelivery, sqlx::Error> {
        Ok(Redelivery::NotFound)
    }

    /// Delete up to `limit` outbox deliveries that ended (`delivered` or
    /// `failed`) more than `days` days ago, and up to `limit` attempt rows
    /// (`webhook_deliveries`) made more than `days` days ago, on the
    /// database's clock: the retention sweep's batch. A pending delivery is
    /// never deleted, however old. Returns how many of each it deleted.
    async fn purge_webhook_log(&self, _days: u32, _limit: u32) -> Result<WebhookLogPurged, sqlx::Error> {
        Ok(WebhookLogPurged::default())
    }

    /// Delete up to `limit` request artifacts (`request_artifacts`: a
    /// request's prompt, params and output) made before `before`, the oldest
    /// first: one batch of the retention sweep's purge of the request log
    /// (`logs.artifact_retention_days`). Their log rows stay. Returns how many
    /// it deleted.
    async fn purge_request_artifacts(&self, _before: chrono::DateTime<chrono::Utc>, _limit: u32) -> Result<u64, sqlx::Error> {
        Ok(0)
    }

    /// Delete up to `limit` request log rows made before `before`, the oldest
    /// first, each with its spend tags (`request_log_tags`): one batch of the
    /// retention sweep's purge of the request log (`logs.retention_days`).
    /// Never a generation row, though an async job's log row has its id.
    /// Returns how many log rows it deleted.
    async fn purge_request_logs(&self, _before: chrono::DateTime<chrono::Utc>, _limit: u32) -> Result<u64, sqlx::Error> {
        Ok(0)
    }

    /// Whether the database takes one writer at a time (SQLite), so a long
    /// run of writes holds up every other: the retention sweep's purges of
    /// the request log and the webhook log then wait between their batches
    /// (`proxy::retention::SINGLE_WRITER_PAUSE`). Postgres locks only the rows
    /// a write touches.
    fn single_writer(&self) -> bool {
        false
    }

    // ─── Request Artifacts ──────────────────────────────────────────────

    /// Store a request artifact (fire-and-forget; spawned async).
    /// Default impl is a no-op so test mocks don't break.
    async fn insert_request_artifact(&self, _a: &RequestArtifact) -> Result<(), sqlx::Error> {
        Ok(())
    }

    /// Look up a request artifact by its request_id.
    /// Default impl returns None (for mock/test implementations).
    async fn get_request_artifact(&self, _request_id: &str) -> Result<Option<RequestArtifact>, sqlx::Error> {
        Ok(None)
    }

    // ─── Management ─────────────────────────────────────────────────────

    /// The management-API store (limits, end users, spend, runtime config),
    /// when this backend has one. Test doubles keep the `None` default and the
    /// management endpoints answer 501 against them.
    fn management(&self) -> Option<&dyn crate::management::store::ManagementStore> {
        None
    }

    /// The MCP OAuth store (clients, requests, grants, tokens), when this
    /// backend has one. Test doubles keep the `None` default, and MCP then
    /// offers no OAuth.
    fn mcp_oauth(&self) -> Option<&dyn crate::mcp::oauth::store::McpOAuthStore> {
        None
    }

    /// Where billed requests record what they reserve (migration 030), when
    /// this backend keeps it. Test doubles keep the `None` default, and their
    /// requests reserve unrecorded, as before 030.
    fn reservations(&self) -> Option<&dyn crate::management::reservations::ReservationStore> {
        None
    }

    // ─── Liveness ───────────────────────────────────────────────────────

    /// Liveness ping: execute `SELECT 1`. Returns Ok(()) if DB is reachable.
    /// Default impl returns Ok(()) (for mock/test implementations).
    async fn ping(&self) -> Result<(), sqlx::Error> {
        Ok(())
    }

    // ─── Users ──────────────────────────────────────────────────────────

    async fn create_user(&self, _user: &User) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn get_user_by_email(&self, _email: &str) -> Result<Option<User>, sqlx::Error> {
        Ok(None)
    }

    async fn get_user_by_id(&self, _id: &str) -> Result<Option<User>, sqlx::Error> {
        Ok(None)
    }

    async fn get_user_by_oauth(&self, _provider: &str, _oauth_id: &str) -> Result<Option<User>, sqlx::Error> {
        Ok(None)
    }

    async fn update_user(
        &self,
        _id: &str,
        _role: Option<Role>,
        _is_active: Option<bool>,
        _password_hash: Option<&str>,
    ) -> Result<Option<User>, sqlx::Error> {
        Ok(None)
    }

    /// [`update_user`](Self::update_user), also handing back, for the audit
    /// log, the pending invitations a deactivation revoked in its
    /// transaction (review round 6, finding 2). With `role` and `member_org`,
    /// the account's membership of `member_org` takes the role in the same
    /// transaction (review round 7, finding 3): single-tenant permissions
    /// come from the default org's membership, so the two roles move together
    /// or not at all. This default revokes none, and writes the membership
    /// after the account.
    async fn update_user_with_revoked(
        &self,
        id: &str,
        role: Option<Role>,
        is_active: Option<bool>,
        password_hash: Option<&str>,
        member_org: Option<&str>,
    ) -> Result<Option<(User, Vec<Invitation>)>, sqlx::Error> {
        let Some(user) = self.update_user(id, role, is_active, password_hash).await? else { return Ok(None) };
        if let (Some(role), Some(org)) = (role, member_org) {
            self.update_member_role(org, id, role).await?;
        }
        Ok(Some((user, Vec::new())))
    }

    async fn touch_last_login(&self, _id: &str) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn count_users(&self) -> Result<i64, sqlx::Error> {
        Ok(0)
    }

    /// The first single-tenant signup: `user`, the deployment's Owner, and
    /// its Owner membership of `org_id`, made together and only while no
    /// account exists. `Ok(false)`, with nothing written, when an account
    /// exists or another first signup won; a failed write leaves no account,
    /// so signup stays open. The SQL stores do it in one transaction that
    /// first claims the one row of `first_owner` (migration 034): of first
    /// signups at once, on any replica, the later claims conflict. This
    /// default, for stores without one, takes the steps one at a time.
    async fn create_first_owner(&self, user: &User, org_id: &str) -> Result<bool, sqlx::Error> {
        if self.count_users().await? > 0 {
            return Ok(false);
        }
        self.create_user(user).await?;
        self.add_org_member(org_id, &user.id, Role::Owner).await?;
        Ok(true)
    }

    async fn list_users(&self) -> Result<Vec<User>, sqlx::Error> {
        Ok(vec![])
    }

    async fn link_oauth(&self, _user_id: &str, _provider: &str, _oauth_id: &str) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn transfer_owner(&self, _new_owner_id: &str) -> Result<(), sqlx::Error> {
        Ok(())
    }

    // ─── Sessions ───────────────────────────────────────────────────────
    //
    // A session is kept by its token's hash (migration 044), so a copy of
    // the database holds no session that works. Only `create_session`,
    // `create_session_at_epoch` (handed the fresh token, as `Session::id`)
    // and `get_session_with_user` (handed the cookie) hash. Every other
    // method, and `SignIn::Session`, takes the stored id that
    // `get_session_with_user` hands back, and hashes nothing.

    /// Store `s`, kept by the hash of its token: `s.id` is the token the
    /// `litegen_session` cookie carries, and the row's id is its
    /// [`token_hash`](crate::auth::tokens::token_hash).
    async fn create_session(&self, _s: &Session) -> Result<(), sqlx::Error> {
        Ok(())
    }

    /// Insert `s` stamped with the sign-in epoch its sign-in began under
    /// (read when it began, before checking the password): a reset, a
    /// deactivation or a password change that committed meanwhile moved the
    /// account's epoch on, and the session check refuses it (review round 4,
    /// finding 2). [`create_session`](Self::create_session) stamps the epoch
    /// the account has at the insert.
    async fn create_session_at_epoch(&self, s: &Session, _epoch: i64) -> Result<(), sqlx::Error> {
        self.create_session(s).await
    }

    /// The account's sign-in epoch (`None`: no such account). It moves on,
    /// in the same transaction, with every confirmed reset, deactivation and
    /// change of one's own password.
    async fn sign_in_epoch(&self, _user_id: &str) -> Result<Option<i64>, sqlx::Error> {
        Ok(Some(0))
    }

    /// The sign-in epoch session `id` was stamped with (`None`: no such
    /// session); an OAuth approval records its session's.
    async fn session_epoch(&self, _id: &str) -> Result<Option<i64>, sqlx::Error> {
        Ok(Some(0))
    }

    /// Change one's own password (review round 4, finding 6; round 5,
    /// finding 1): write `password_hash`, only while the account still holds
    /// `verified_hash` (the one the current password was checked against), is
    /// active, and is at the sign-in epoch of `keep_session` (the stored id
    /// of the session sending the change); move the epoch on and end the other sign-ins
    /// (sessions but `keep_session`, OAuth grants with their tokens, codes
    /// not yet redeemed, unused links it issued) in the same transaction,
    /// re-stamping `keep_session` with the new epoch so it stays valid.
    async fn change_own_password(&self, user_id: &str, password_hash: &str, _verified_hash: &str, keep_session: &str) -> Result<PasswordChange, sqlx::Error> {
        if self.update_user(user_id, None, None, Some(password_hash)).await?.is_none() {
            return Ok(PasswordChange::SignInEnded);
        }
        self.delete_user_sessions(user_id, Some(keep_session)).await?;
        Ok(PasswordChange::Done)
    }

    /// The session stored as `id` (the hash of its token).
    async fn get_session(&self, _id: &str) -> Result<Option<Session>, sqlx::Error> {
        Ok(None)
    }

    /// The session whose token is `cookie` (the `litegen_session` cookie),
    /// found by the cookie's hash and handed back with its stored id, and
    /// its user (`None` when the user is gone): what authenticating a
    /// session needs, in one statement where the store can (review
    /// 2026-11-11, P14). A stored id sent as the cookie finds nothing; there
    /// is no fallback to the plain value, which would make a leaked hash (64
    /// hex digits, like a token) a cookie. This default runs the two lookups.
    async fn get_session_with_user(&self, cookie: &str) -> Result<Option<(Session, Option<User>)>, sqlx::Error> {
        let Some(session) = self.get_session(&crate::auth::tokens::token_hash(cookie)).await? else { return Ok(None) };
        let user = self.get_user_by_id(&session.user_id).await?;
        Ok(Some((session, user)))
    }

    async fn delete_session(&self, _id: &str) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn delete_user_sessions(&self, _user_id: &str, _except_id: Option<&str>) -> Result<u64, sqlx::Error> {
        Ok(0)
    }

    async fn list_user_sessions(&self, _user_id: &str) -> Result<Vec<Session>, sqlx::Error> {
        Ok(vec![])
    }

    async fn bump_session_expiry(
        &self,
        _id: &str,
        _new_expires_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), sqlx::Error> {
        Ok(())
    }

    // ─── Invitations ────────────────────────────────────────────────────

    /// Store `inv`, unconditionally: for the master key's invitations (and
    /// tests). A person's request stores its invitation with
    /// [`create_invitation_from`](Self::create_invitation_from).
    async fn create_invitation(&self, _inv: &Invitation) -> Result<(), sqlx::Error> {
        Ok(())
    }

    /// Store `inv` only while `from` is live, checked in the statement that
    /// stores it (review round 6, finding 2): `Ok(false)`, with nothing
    /// stored, when it has ended. This default checks nothing.
    async fn create_invitation_from(&self, inv: &Invitation, _from: SignIn<'_>) -> Result<bool, sqlx::Error> {
        self.create_invitation(inv).await.map(|()| true)
    }

    async fn get_invitation(&self, _token: &str) -> Result<Option<Invitation>, sqlx::Error> {
        Ok(None)
    }

    async fn mark_invitation_used(&self, _token: &str) -> Result<bool, sqlx::Error> {
        Ok(true)
    }

    async fn delete_invitation(&self, _id: &str) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn list_invitations(&self) -> Result<Vec<Invitation>, sqlx::Error> {
        Ok(vec![])
    }

    /// The org's invitations not yet accepted, newest first. Expired ones
    /// are among them: the caller judges expiry on its own clock, as
    /// accepting does.
    async fn list_unused_invitations(&self, _org_id: &str) -> Result<Vec<Invitation>, sqlx::Error> {
        Ok(vec![])
    }

    /// Delete the org's invitation `id` while it is not yet accepted, and
    /// return it; `None` when the org has no such invitation.
    async fn revoke_invitation(&self, _org_id: &str, _id: &str) -> Result<Option<Invitation>, sqlx::Error> {
        Ok(None)
    }

    /// Whether the org's invitation `id` was accepted (a revoke answers 409
    /// for it, 404 for one the org doesn't have).
    async fn invitation_accepted(&self, _org_id: &str, _id: &str) -> Result<bool, sqlx::Error> {
        Ok(false)
    }

    /// Accept the invitation of `token` (as handed out; the store looks it
    /// up by its hash): consume it, while
    /// it is neither accepted nor revoked, with the invitee's new account
    /// (`new_user`, when they had none) and their membership of `org_id` as
    /// `role`, all in one transaction. `false`, with nothing written, when it
    /// was accepted or revoked meanwhile. This default takes the steps one at
    /// a time, consuming last.
    async fn accept_invitation(&self, token: &str, new_user: Option<&User>, user_id: &str, org_id: &str, role: Role) -> Result<bool, sqlx::Error> {
        if let Some(user) = new_user {
            self.create_user(user).await?;
        }
        if self.get_membership(org_id, user_id).await?.is_none() {
            self.add_org_member(org_id, user_id, role).await?;
        }
        self.mark_invitation_used(token).await
    }

    /// Delete the pending (unused) invitations, in `org_id` or in every org,
    /// that go to `email` or that `invited_by` sent (SEC-INVITES): a removed
    /// or deactivated person, or one moved to a role that can't invite, keeps
    /// no way back in. Emails are stored lower-cased. Returns the ones that
    /// went, for the audit log: the one store helper behind every revoke
    /// (review round 6, finding 6), which a deactivation and a confirmed
    /// reset also run in their own transactions.
    async fn revoke_pending_invitations(
        &self,
        _org_id: Option<&str>,
        _email: Option<&str>,
        _invited_by: Option<&str>,
    ) -> Result<Vec<Invitation>, sqlx::Error> {
        Ok(vec![])
    }

    // ─── Password Resets ────────────────────────────────────────────────

    /// Store the reset link `r`, voiding the account's older unused ones,
    /// unconditionally: for the master key's links, the email self-service
    /// flow's (and tests). A person's request stores its link with
    /// [`create_password_reset_from`](Self::create_password_reset_from).
    async fn create_password_reset(&self, _r: &PasswordReset) -> Result<(), sqlx::Error> {
        Ok(())
    }

    /// Store the reset link `r`, voiding the account's older unused ones,
    /// only while `from` is live, checked in the statement that stores it
    /// (review round 6, finding 2): `Ok(false)`, with nothing changed, when
    /// it has ended. This default checks nothing.
    async fn create_password_reset_from(&self, r: &PasswordReset, _from: SignIn<'_>) -> Result<bool, sqlx::Error> {
        self.create_password_reset(r).await.map(|()| true)
    }

    /// Use the reset of `token` (as handed out): consume it (the single-use
    /// CAS) and set the account's password to `password_hash`, in one
    /// transaction, only while the account is active. This default takes
    /// the steps one at a time, writing the password first.
    async fn confirm_password_reset(&self, token: &str, password_hash: &str, _mode: crate::config::Mode) -> Result<ResetConfirm, sqlx::Error> {
        let Some(reset) = self.get_password_reset(token).await? else { return Ok(ResetConfirm::Unusable) };
        self.update_user(&reset.user_id, None, None, Some(password_hash)).await?;
        Ok(if self.mark_password_reset_used(token).await? { ResetConfirm::Done { user_id: reset.user_id, reactivated: false, revoked_invitations: vec![] } } else { ResetConfirm::Unusable })
    }

    async fn get_password_reset(&self, _token: &str) -> Result<Option<PasswordReset>, sqlx::Error> {
        Ok(None)
    }

    /// Atomically mark a reset token used. Returns `true` if this call won the
    /// transition (token was unused), `false` if it was already used — callers
    /// must only apply the password change when this returns `true`.
    async fn mark_password_reset_used(&self, _token: &str) -> Result<bool, sqlx::Error> {
        Ok(true)
    }

    // ─── Login Attempts ─────────────────────────────────────────────────

    async fn record_login_attempt(&self, _email: &str, _success: bool) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn recent_failed_login_attempts(
        &self,
        _email: &str,
        _since: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<chrono::DateTime<chrono::Utc>>, sqlx::Error> {
        Ok(vec![])
    }

    // ─── Organizations ──────────────────────────────────────────────────
    // Default impls return empty/Ok/None/false so mock/test implementations
    // (and backends not yet migrated) still compile.

    async fn create_organization(&self, _o: &Organization) -> Result<(), sqlx::Error> {
        Ok(())
    }

    /// `org`, its Owner's membership and its first `app`, made together,
    /// with the Owner's account too when `owner` is [`OrgOwner::New`] (a
    /// hosted signup): a failed write leaves none of them. This default
    /// takes the steps one at a time.
    async fn create_org_with_owner(&self, org: &Organization, app: &Application, owner: OrgOwner<'_>) -> Result<(), sqlx::Error> {
        if let OrgOwner::New(user) = owner {
            self.create_user(user).await?;
        }
        self.create_organization(org).await?;
        self.add_org_member(&org.id, owner.user_id(), Role::Owner).await?;
        self.create_application(app).await?;
        Ok(())
    }

    async fn get_organization(&self, _id: &str) -> Result<Option<Organization>, sqlx::Error> {
        Ok(None)
    }

    async fn get_org_by_slug(&self, _slug: &str) -> Result<Option<Organization>, sqlx::Error> {
        Ok(None)
    }

    /// Organizations the user belongs to, paired with their role in each.
    async fn list_orgs_for_user(
        &self,
        _user_id: &str,
    ) -> Result<Vec<(Organization, Role)>, sqlx::Error> {
        Ok(vec![])
    }

    async fn update_organization(
        &self,
        _id: &str,
        _name: Option<&str>,
    ) -> Result<Option<Organization>, sqlx::Error> {
        Ok(None)
    }

    async fn delete_organization(&self, _id: &str) -> Result<bool, sqlx::Error> {
        Ok(false)
    }

    // ─── Members ────────────────────────────────────────────────────────

    async fn add_org_member(
        &self,
        _org_id: &str,
        _user_id: &str,
        _role: Role,
    ) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn get_membership(
        &self,
        _org_id: &str,
        _user_id: &str,
    ) -> Result<Option<Role>, sqlx::Error> {
        Ok(None)
    }

    async fn list_org_members(&self, _org_id: &str) -> Result<Vec<OrganizationMember>, sqlx::Error> {
        Ok(vec![])
    }

    async fn update_member_role(
        &self,
        _org_id: &str,
        _user_id: &str,
        _role: Role,
    ) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn remove_org_member(&self, _org_id: &str, _user_id: &str) -> Result<(), sqlx::Error> {
        Ok(())
    }

    /// Demote the current owner(s) to admin and promote `new_owner_user_id` to owner.
    /// The default refuses, as `transfer_deployment_owner`'s does.
    async fn transfer_org_owner(
        &self,
        _org_id: &str,
        _new_owner_user_id: &str,
    ) -> Result<(), sqlx::Error> {
        Err(sqlx::Error::Protocol("transfer_org_owner is not implemented by this store".into()))
    }

    /// Single-tenant: hand the deployment's ownership to `new_owner_user_id`
    /// in both places it is recorded, in one transaction. One is the default
    /// org's membership (`org_id`), where permissions come from; the other is
    /// `users.role`. Each current Owner becomes an Admin in both. Fails with
    /// `RowNotFound`, changing nothing, when the new owner is not a member of
    /// `org_id`. The default refuses, so a store that doesn't implement it
    /// can never answer "transferred" having moved nothing.
    async fn transfer_deployment_owner(
        &self,
        _org_id: &str,
        _new_owner_user_id: &str,
    ) -> Result<(), sqlx::Error> {
        Err(sqlx::Error::Protocol("transfer_deployment_owner is not implemented by this store".into()))
    }

    // ─── Applications ───────────────────────────────────────────────────

    async fn create_application(&self, _a: &Application) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn get_application(&self, _id: &str) -> Result<Option<Application>, sqlx::Error> {
        Ok(None)
    }

    async fn list_apps_for_org(&self, _org_id: &str) -> Result<Vec<Application>, sqlx::Error> {
        Ok(vec![])
    }

    async fn update_application(
        &self,
        _id: &str,
        _name: Option<&str>,
    ) -> Result<Option<Application>, sqlx::Error> {
        Ok(None)
    }

    async fn delete_application(&self, _id: &str) -> Result<bool, sqlx::Error> {
        Ok(false)
    }

    // ─── Provider Credentials (org-scoped; store opaque ciphertext only) ────

    async fn upsert_org_provider_credential(
        &self,
        _org_id: &str,
        _provider: &str,
        _ciphertext: &str,
        _nonce: &str,
        _display_hint: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        Ok(())
    }

    /// Returns `(ciphertext, nonce)` for the stored credential, if any.
    async fn get_org_provider_credential(
        &self,
        _org_id: &str,
        _provider: &str,
    ) -> Result<Option<(String, String)>, sqlx::Error> {
        Ok(None)
    }

    async fn list_org_provider_credentials(
        &self,
        _org_id: &str,
    ) -> Result<Vec<ProviderCredentialInfo>, sqlx::Error> {
        Ok(vec![])
    }

    async fn delete_org_provider_credential(
        &self,
        _org_id: &str,
        _provider: &str,
    ) -> Result<bool, sqlx::Error> {
        Ok(false)
    }

    // ─── Per-app BYO storage config ─────────────────────────────────────

    async fn upsert_app_storage(&self, _input: &AppStorageUpsert) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn get_app_storage(&self, _app_id: &str) -> Result<Option<AppStorageRow>, sqlx::Error> {
        Ok(None)
    }

    async fn delete_app_storage(&self, _app_id: &str) -> Result<bool, sqlx::Error> {
        Ok(false)
    }

    // ─── Per-app stored-file access overrides ───────────────────────────

    async fn get_app_file_access(&self, _app_id: &str) -> Result<Option<crate::types::AppFileAccessRow>, sqlx::Error> {
        Ok(None)
    }

    /// Generation `id`'s app and that app's file access settings, or `None`
    /// when there is no such generation: what serving a stored file needs to
    /// decide who may open it, in one statement where the store can (review
    /// 2026-11-11, P14). This default runs the two lookups.
    async fn generation_file_access(&self, id: &str) -> Result<Option<(Option<String>, Option<crate::types::AppFileAccessRow>)>, sqlx::Error> {
        let Some(gen) = self.get_generation(id).await? else { return Ok(None) };
        let access = match gen.app_id.as_deref() {
            Some(app) => self.get_app_file_access(app).await?,
            None => None,
        };
        Ok(Some((gen.app_id, access)))
    }

    /// Replace the app's overrides: every field, `None` included.
    async fn put_app_file_access(&self, _input: &crate::types::AppFileAccessUpsert) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn delete_app_file_access(&self, _app_id: &str) -> Result<bool, sqlx::Error> {
        Ok(false)
    }

    // ─── Retention ──────────────────────────────────────────────────────

    /// Apps with a `retention_days` of their own (0 included: kept until deleted).
    async fn apps_with_retention(&self) -> Result<Vec<(String, i64)>, sqlx::Error> {
        Ok(Vec::new())
    }

    /// Up to `limit` finished generations created before `cutoff` whose files
    /// are still kept: `app_id`'s, or with `None`, those of every app without
    /// a retention of its own (and of no app). The rows the sweep has not
    /// tried come first, oldest first; then those whose delete failed
    /// ([`Self::note_files_delete_failed`]), least recently tried first.
    async fn generations_due_for_retention(
        &self,
        _app_id: Option<&str>,
        _cutoff: chrono::DateTime<chrono::Utc>,
        _limit: i64,
    ) -> Result<Vec<Generation>, sqlx::Error> {
        Ok(Vec::new())
    }

    /// Record that a generation's kept files were deleted, and why.
    async fn mark_files_deleted(&self, _id: &str, _at: chrono::DateTime<chrono::Utc>, _reason: &str) -> Result<(), sqlx::Error> {
        Ok(())
    }

    /// Clear the request log's copy of request `request_id`'s output, whose
    /// files were deleted: its artifact's `output_value` goes and its
    /// `output_kind` becomes `deleted`. Its prompt and params stay. Only an
    /// output that holds something (`b64`: an image's base64, up to 2 MiB;
    /// `url`) changes.
    async fn clear_artifact_output(&self, _request_id: &str) -> Result<(), sqlx::Error> {
        Ok(())
    }

    /// Record that the retention sweep failed to delete a generation's files
    /// just now, on the database's clock.
    async fn note_files_delete_failed(&self, _id: &str) -> Result<(), sqlx::Error> {
        Ok(())
    }

    /// Remove a generation's request (its prompt and params) from the row, and
    /// its request log's copies (`request_artifacts`): its own artifact and
    /// those of the cache hits it answered (`output_kind` `generation` naming
    /// it), which carry the same prompt and params. The rest of the row stays,
    /// and so do the log rows (the billing records).
    async fn redact_generation_request(&self, _id: &str) -> Result<(), sqlx::Error> {
        Ok(())
    }

    // ─── A deleted org's or app's files (migration 031) ─────────────────

    /// Claim up to `limit` of the due file deletions that deleting an org or
    /// an app recorded (with `of`, `(scope, id)`, only that org's or app's),
    /// the longest due first. In one statement each one's attempts count up
    /// and its next attempt moves `hold` ahead on the database's clock, so no
    /// other claim takes it until then: a worker that dies leaves it due again
    /// once `hold` has passed.
    async fn claim_tenant_file_deletions(
        &self,
        _of: Option<(&str, &str)>,
        _limit: u32,
        _hold: chrono::Duration,
    ) -> Result<Vec<TenantFileDeletion>, sqlx::Error> {
        Ok(Vec::new())
    }

    /// The file of deletion `id` is gone: remove its record. Returns whether
    /// there was one.
    async fn finish_tenant_file_deletion(&self, _id: i64) -> Result<bool, sqlx::Error> {
        Ok(false)
    }

    /// The delete of deletion `id` failed: due again `after` from the
    /// database's now, with `error`. Only while its attempts are still
    /// `attempts`, the claim's: one another worker has claimed since is that
    /// worker's. Returns whether it did.
    async fn retry_tenant_file_deletion(&self, _id: i64, _attempts: i32, _after: chrono::Duration, _error: &str) -> Result<bool, sqlx::Error> {
        Ok(false)
    }

    /// Record `urls`, files of generation `generation_id` (of `scope`
    /// `tenant_id`) whose row is gone, for deletion as a delete's own records
    /// are: what a job's delivery stored after its org or app was deleted.
    async fn record_tenant_file_deletions(&self, _scope: &str, _tenant_id: &str, _generation_id: &str, _urls: &[String]) -> Result<(), sqlx::Error> {
        Ok(())
    }

    /// Remove what one request of a deleted org or app wrote after the delete
    /// (it ran while the tenant was deleted): its request-log row, tags and
    /// artifact (`request_id`), and its generation row (`generation_id`), in
    /// one transaction.
    async fn forget_request_rows(&self, _generation_id: Option<&str>, _request_id: &str) -> Result<(), sqlx::Error> {
        Ok(())
    }

    /// Whether every row a request writes references its org and app through
    /// a foreign key: its log row, artifact and generation name them, and
    /// each spend tag its log row (Postgres, migration 037). A row written
    /// after its tenant's delete is then refused, and one written while the
    /// delete runs makes it start again and take it. SQLite's tenant columns
    /// carry no keys.
    fn request_rows_reference_their_tenant(&self) -> bool {
        false
    }

    /// Say that request `request_id` was answered from the cache (migration
    /// 036): its log row's `cache_hit`.
    async fn mark_cache_hit(&self, _request_id: &str) -> Result<(), sqlx::Error> {
        Ok(())
    }

    // ─── Revoked links ──────────────────────────────────────────────────

    /// Refuse every link litegen signed before `at` in a scope: `gateway`
    /// (scope_id `""`), `app` or `generation`. An earlier time never replaces
    /// a later one.
    async fn revoke_links(&self, _scope: &str, _scope_id: &str, _at: chrono::DateTime<chrono::Utc>) -> Result<(), sqlx::Error> {
        Ok(())
    }

    /// The latest revocation that covers a link to `generation_id`'s files:
    /// the gateway's, its app's (when it has one) and its own.
    async fn links_revoked_at(&self, _app_id: Option<&str>, _generation_id: &str) -> Result<Option<chrono::DateTime<chrono::Utc>>, sqlx::Error> {
        Ok(None)
    }

    // ─── Server secrets ─────────────────────────────────────────────────

    /// The server secret `name`: stored from `generate()` if nobody has stored
    /// one yet, and the same stored value ever after, whichever replica asks
    /// and however many ask at once (the first write wins; `generate` runs on
    /// every call, and its output is dropped when a value is already stored).
    ///
    /// The default makes a new value on every call, which is all a test double
    /// needs. A production store must override it, or every replica keeps its
    /// own secret.
    async fn server_secret(&self, _name: &str, generate: &(dyn Fn() -> String + Send + Sync)) -> Result<String, sqlx::Error> {
        Ok(generate())
    }

    // ─── Model Allowlists ───────────────────────────────────────────────────────

    async fn get_org_allowed_models(&self, _org_id: &str) -> Result<Vec<String>, sqlx::Error> {
        Ok(vec![])
    }

    async fn set_org_allowed_models(
        &self,
        _org_id: &str,
        _models: &[String],
    ) -> Result<(), sqlx::Error> {
        Ok(())
    }

    /// Returns `(mode, model_ids)`. `mode` defaults to `"all"` if no row exists.
    async fn get_app_model_access(
        &self,
        _app_id: &str,
    ) -> Result<(String, Vec<String>), sqlx::Error> {
        Ok(("all".to_string(), vec![]))
    }

    async fn set_app_model_access(
        &self,
        _app_id: &str,
        _mode: &str,
        _models: &[String],
    ) -> Result<(), sqlx::Error> {
        Ok(())
    }

    // ─── API Keys (tenant-scoped create / list) ─────────────────────────

    async fn create_api_key_scoped(
        &self,
        _org_id: &str,
        _app_id: &str,
        _public_id: &str,
        _name: &str,
        _key_hash: &str,
        _key_prefix: &str,
        _token_quota: Option<f64>,
        _rpm_limit: Option<u32>,
        _scopes: &str,
        _webhook_url: Option<&str>,
        _expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<ApiKey, sqlx::Error> {
        Err(sqlx::Error::RowNotFound)
    }

    async fn list_api_keys_for_app(&self, _app_id: &str) -> Result<Vec<ApiKey>, sqlx::Error> {
        Ok(vec![])
    }

    // ─── Tenant-scoped reads ────────────────────────────────────────────

    async fn list_generations_for_tenant(
        &self,
        _org_id: &str,
        _app_id: Option<&str>,
        _page: u32,
        _per_page: u32,
    ) -> Result<Vec<Generation>, sqlx::Error> {
        Ok(vec![])
    }

    async fn count_generations_for_tenant(
        &self,
        _org_id: &str,
        _app_id: Option<&str>,
    ) -> Result<i64, sqlx::Error> {
        Ok(0)
    }

    /// The org's generations still `pending` or `processing` (with `app_id`,
    /// one app's): the jobs deleting it would leave running (SEC-INMEM).
    async fn list_active_generations_for_tenant(
        &self,
        _org_id: &str,
        _app_id: Option<&str>,
    ) -> Result<Vec<Generation>, sqlx::Error> {
        Ok(vec![])
    }

    async fn get_request_logs_for_tenant(
        &self,
        _org_id: &str,
        _app_id: Option<&str>,
        _page: u32,
        _per_page: u32,
    ) -> Result<(Vec<RequestLog>, u64), sqlx::Error> {
        Ok((Vec::new(), 0))
    }

    async fn list_audit_log_for_tenant(
        &self,
        _org_id: &str,
        _page: u32,
        _per_page: u32,
    ) -> Result<(Vec<AuditLogEntry>, i64), sqlx::Error> {
        Ok((Vec::new(), 0))
    }

    /// Tenant-scoped, filtered, paginated request logs — filtered and counted at
    /// the DB so a filtered or deep-page query returns the true matching set and
    /// total, not a slice of a truncated in-memory window. `from`/`to` are bound
    /// as real timestamps (correct on Postgres TIMESTAMPTZ; lexicographic on the
    /// fixed-width SQLite text column).
    async fn get_request_logs_for_tenant_filtered(
        &self,
        _org_id: &str,
        _app_id: Option<&str>,
        _filter: &RequestLogFilter<'_>,
        _page: u32,
        _per_page: u32,
    ) -> Result<(Vec<RequestLog>, u64), sqlx::Error> {
        Ok((Vec::new(), 0))
    }

    /// Tenant-scoped, filtered, paginated audit log (see
    /// `get_request_logs_for_tenant_filtered`): the entries filed under one
    /// org, or under none ([`AuditLogOrg`]).
    async fn list_audit_log_for_tenant_filtered(
        &self,
        _org: AuditLogOrg<'_>,
        _filter: &TenantAuditFilter<'_>,
        _page: u32,
        _per_page: u32,
    ) -> Result<(Vec<AuditLogEntry>, u64), sqlx::Error> {
        Ok((Vec::new(), 0))
    }

    /// Paginated generations owned by a specific set of API keys within a tenant,
    /// filtered and counted at the DB layer. Returns `(rows, total_matching)` so
    /// a read-own member sees their complete history — not just whichever of
    /// their rows happen to fall inside a truncated in-memory window.
    async fn list_generations_for_owner(
        &self,
        _org_id: &str,
        _app_id: Option<&str>,
        _key_ids: &[Uuid],
        _page: u32,
        _per_page: u32,
    ) -> Result<(Vec<Generation>, u64), sqlx::Error> {
        Ok((Vec::new(), 0))
    }
}
