//! [`McpOAuthStore`] contract tests, run against every backend: SQLite
//! always, Postgres when `LITEGEN_PG_TEST_URL` is set (like
//! `management::store_tests`). Ids are unique per run so a shared Postgres
//! database can be reused.

use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use super::store::*;
use crate::db::DatabaseStore;
use crate::mcp::Surface;

fn uid(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4().simple())
}

/// Now, at the precision every backend stores. Postgres keeps microseconds,
/// so on Linux, whose clock has nanoseconds, a row built from `Utc::now()`
/// wouldn't read back equal.
fn stored_now() -> DateTime<Utc> {
    use chrono::SubsecRound;
    Utc::now().trunc_subsecs(6)
}

fn store(db: &dyn DatabaseStore) -> &dyn McpOAuthStore {
    db.mcp_oauth().expect("backend has an OAuth store")
}

fn request(client_id: &str, now: DateTime<Utc>) -> AuthRequest {
    AuthRequest {
        id: uid("req"),
        client_id: client_id.to_string(),
        redirect_uri: "http://127.0.0.1/callback".into(),
        state: Some("st".into()),
        code_challenge: "challenge".into(),
        scopes: vec!["ai:generate".into(), "ai:read".into()],
        surface: Surface::Ai,
        created_at: now,
        expires_at: now + Duration::minutes(10),
        user_id: None,
        org_id: None,
        app_id: None,
        granted_scopes: None,
        code_hash: None,
        decided_at: None,
        consumed_at: None,
    }
}

fn approve(code_hash: &str, now: DateTime<Utc>) -> Decision {
    Decision::Approve {
        user_id: "user-1".into(),
        org_id: "org-1".into(),
        app_id: Some("app-1".into()),
        scopes: vec!["ai:generate".into()],
        code_hash: code_hash.to_string(),
        code_expires_at: now + Duration::seconds(60),
        at: now,
    }
}

fn grant(client_id: &str, user_id: &str, now: DateTime<Utc>) -> Grant {
    Grant {
        id: uid("grant"),
        client_id: client_id.to_string(),
        user_id: user_id.to_string(),
        org_id: "org-1".into(),
        app_id: Some("app-1".into()),
        surface: Surface::Ai,
        scopes: vec!["ai:generate".into(), "ai:read".into()],
        created_at: now,
        last_used_at: None,
        revoked_at: None,
    }
}

fn token(grant_id: &str, kind: TokenKind, now: DateTime<Utc>) -> TokenRow {
    TokenRow {
        token_hash: uid("hash"),
        grant_id: grant_id.to_string(),
        kind,
        created_at: now,
        expires_at: now + Duration::hours(1),
        rotated_at: None,
        revoked_at: None,
    }
}

fn client(registration: Registration, at: DateTime<Utc>) -> OAuthClient {
    let client_id = match registration {
        Registration::Dcr => uid("lgc"),
        Registration::Cimd => format!("https://{}.example.com/client.json", uid("c")),
    };
    OAuthClient {
        client_id,
        registration,
        client_name: "Test client".into(),
        redirect_uris: vec!["http://127.0.0.1/callback".into()],
        logo_uri: None,
        client_uri: None,
        metadata: serde_json::json!({ "client_name": "Test client" }),
        created_at: at,
        refreshed_at: at,
        last_used_at: None,
    }
}

/// An org with one app, as real rows: purging keeps only the grants whose
/// org and app still exist.
async fn org_and_app(db: &dyn DatabaseStore) -> (String, String) {
    let now = stored_now();
    let (org, app) = (uid("org"), uid("app"));
    db.create_organization(&crate::types::Organization {
        id: org.clone(),
        name: "Store Test".into(),
        slug: org.clone(),
        plan: "free".into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    })
    .await
    .unwrap();
    db.create_application(&crate::types::Application {
        id: app.clone(),
        org_id: org.clone(),
        name: "Store Test".into(),
        slug: app.clone(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    })
    .await
    .unwrap();
    (org, app)
}

/// A grant in a real org and app.
fn grant_in(org: &str, app: &str, client_id: &str, user_id: &str, now: DateTime<Utc>) -> Grant {
    Grant { org_id: org.to_string(), app_id: Some(app.to_string()), ..grant(client_id, user_id, now) }
}

async fn clients_round_trip(db: &dyn DatabaseStore) {
    let s = store(db);
    let now = stored_now();
    let mut client = OAuthClient {
        client_id: uid("lgc"),
        registration: Registration::Dcr,
        client_name: "Test client".into(),
        redirect_uris: vec!["http://127.0.0.1/callback".into()],
        logo_uri: None,
        client_uri: Some("https://example.com".into()),
        metadata: serde_json::json!({ "client_name": "Test client" }),
        created_at: now,
        refreshed_at: now,
        last_used_at: None,
    };
    assert!(s.get_client(&client.client_id).await.unwrap().is_none());
    s.upsert_client(&client).await.unwrap();
    let got = s.get_client(&client.client_id).await.unwrap().expect("stored");
    assert_eq!(got.redirect_uris, client.redirect_uris);
    assert_eq!(got.registration, Registration::Dcr);
    assert_eq!(got.last_used_at, None);

    s.touch_client(&client.client_id, now).await.unwrap();
    client.client_name = "Renamed".into();
    client.refreshed_at = now + Duration::hours(1);
    s.upsert_client(&client).await.unwrap();
    let got = s.get_client(&client.client_id).await.unwrap().unwrap();
    assert_eq!(got.client_name, "Renamed");
    assert_eq!(got.created_at.timestamp(), now.timestamp(), "an update keeps the first registration time");
    assert_eq!(got.last_used_at.map(|t| t.timestamp()), Some(now.timestamp()), "and when it was last used");
}

async fn request_approve_then_consume_once(db: &dyn DatabaseStore) {
    let s = store(db);
    let now = stored_now();
    let req = request(&uid("lgc"), now);
    s.insert_request(&req).await.unwrap();
    assert_eq!(s.get_request(&req.id).await.unwrap().as_ref(), Some(&req));

    let code = uid("code");
    assert!(s.decide_request(&req.id, approve(&code, now)).await.unwrap());
    let decided = s.get_request(&req.id).await.unwrap().unwrap();
    assert_eq!(decided.user_id.as_deref(), Some("user-1"));
    assert_eq!(decided.granted_scopes, Some(vec!["ai:generate".to_string()]));

    match s.consume_code(&code, now).await.unwrap() {
        ConsumeOutcome::Consumed(r) => assert_eq!(r.id, req.id),
        other => panic!("first use: {other:?}"),
    }
    assert!(matches!(s.consume_code(&code, now).await.unwrap(), ConsumeOutcome::AlreadyUsed(r) if r.id == req.id));

    // A code nobody redeemed in time is gone.
    let late = request(&uid("lgc"), now);
    s.insert_request(&late).await.unwrap();
    let late_code = uid("code");
    assert!(s.decide_request(&late.id, approve(&late_code, now)).await.unwrap());
    assert_eq!(s.consume_code(&late_code, now + Duration::minutes(2)).await.unwrap(), ConsumeOutcome::Missing);
    assert_eq!(s.consume_code(&uid("code"), now).await.unwrap(), ConsumeOutcome::Missing);
}

async fn decide_is_single_shot(db: &dyn DatabaseStore) {
    let s = store(db);
    let now = stored_now();
    let req = request(&uid("lgc"), now);
    s.insert_request(&req).await.unwrap();
    assert!(s.decide_request(&req.id, Decision::Deny { at: now }).await.unwrap());
    assert!(!s.decide_request(&req.id, approve(&uid("code"), now)).await.unwrap(), "decided already");

    let expired = request(&uid("lgc"), now - Duration::minutes(20));
    s.insert_request(&expired).await.unwrap();
    assert!(!s.decide_request(&expired.id, approve(&uid("code"), now)).await.unwrap(), "expired");
    assert!(!s.decide_request("no-such-request", Decision::Deny { at: now }).await.unwrap());
}

async fn rotate_refresh_detects_reuse(db: &dyn DatabaseStore) {
    let s = store(db);
    let now = stored_now();
    let g = grant(&uid("lgc"), &uid("user"), now);
    s.insert_grant(&g).await.unwrap();
    let a = token(&g.id, TokenKind::Refresh, now);
    s.insert_token(&a).await.unwrap();

    let (access_b, refresh_b) = (token(&g.id, TokenKind::Access, now), token(&g.id, TokenKind::Refresh, now));
    assert_eq!(s.rotate_refresh(&a.token_hash, &access_b, &refresh_b, now).await.unwrap(), RotateOutcome::Rotated);
    assert!(s.get_token(&a.token_hash).await.unwrap().unwrap().rotated_at.is_some());
    assert_eq!(s.get_token(&refresh_b.token_hash).await.unwrap().as_ref(), Some(&refresh_b));
    assert!(s.get_token(&access_b.token_hash).await.unwrap().is_some());

    let (access_c, refresh_c) = (token(&g.id, TokenKind::Access, now), token(&g.id, TokenKind::Refresh, now));
    assert_eq!(
        s.rotate_refresh(&a.token_hash, &access_c, &refresh_c, now).await.unwrap(),
        RotateOutcome::Reused { grant_id: g.id.clone() }
    );
    assert!(s.get_token(&refresh_c.token_hash).await.unwrap().is_none(), "a replay mints nothing");

    // An access token, an expired one and an unknown one are not refreshable.
    assert_eq!(s.rotate_refresh(&access_b.token_hash, &access_c, &refresh_c, now).await.unwrap(), RotateOutcome::Missing);
    assert_eq!(
        s.rotate_refresh(&refresh_b.token_hash, &access_c, &refresh_c, now + Duration::hours(2)).await.unwrap(),
        RotateOutcome::Missing
    );
    assert_eq!(s.rotate_refresh("no-such-token", &access_c, &refresh_c, now).await.unwrap(), RotateOutcome::Missing);
}

async fn revoke_grant_revokes_tokens(db: &dyn DatabaseStore) {
    let s = store(db);
    let now = stored_now();
    let g = grant(&uid("lgc"), &uid("user"), now);
    s.insert_grant(&g).await.unwrap();
    let (access, refresh) = (token(&g.id, TokenKind::Access, now), token(&g.id, TokenKind::Refresh, now));
    s.insert_token(&access).await.unwrap();
    s.insert_token(&refresh).await.unwrap();

    assert!(s.revoke_grant(&g.id, now).await.unwrap());
    assert!(!s.revoke_grant(&g.id, now).await.unwrap(), "already revoked");
    assert!(s.get_grant(&g.id).await.unwrap().unwrap().revoked_at.is_some());
    assert!(s.get_token(&access.token_hash).await.unwrap().unwrap().revoked_at.is_some());
    assert!(s.get_token(&refresh.token_hash).await.unwrap().unwrap().revoked_at.is_some());
    assert_eq!(
        s.rotate_refresh(&refresh.token_hash, &token(&g.id, TokenKind::Access, now), &token(&g.id, TokenKind::Refresh, now), now)
            .await
            .unwrap(),
        RotateOutcome::Missing,
        "a revoked refresh token is dead, not a replay"
    );

    let single = token(&g.id, TokenKind::Access, now);
    s.insert_token(&single).await.unwrap();
    s.revoke_token(&single.token_hash, now).await.unwrap();
    assert!(s.get_token(&single.token_hash).await.unwrap().unwrap().revoked_at.is_some());
}

async fn find_active_grant_ignores_revoked(db: &dyn DatabaseStore) {
    let s = store(db);
    let now = stored_now();
    let (client, user) = (uid("lgc"), uid("user"));
    let g = grant(&client, &user, now);
    s.insert_grant(&g).await.unwrap();
    let found = s.find_active_grant(&client, &user, "org-1", Some("app-1"), Surface::Ai).await.unwrap();
    assert_eq!(found.map(|f| f.id), Some(g.id.clone()));
    assert!(s.find_active_grant(&client, &user, "org-1", Some("app-1"), Surface::Admin).await.unwrap().is_none(), "another endpoint");
    assert!(s.find_active_grant(&client, &user, "org-1", None, Surface::Ai).await.unwrap().is_none(), "another app");

    s.update_grant_scopes(&g.id, &["ai:read".to_string()]).await.unwrap();
    assert_eq!(s.get_grant(&g.id).await.unwrap().unwrap().scopes, vec!["ai:read".to_string()]);
    s.touch_grant(&g.id, now).await.unwrap();
    assert!(s.get_grant(&g.id).await.unwrap().unwrap().last_used_at.is_some());
    assert_eq!(s.list_grants_for_user(&user).await.unwrap().len(), 1);

    s.revoke_grant(&g.id, now).await.unwrap();
    assert!(s.find_active_grant(&client, &user, "org-1", Some("app-1"), Surface::Ai).await.unwrap().is_none());
    assert!(s.list_grants_for_user(&user).await.unwrap().is_empty(), "revoked grants are not listed");

    let appless = Grant { app_id: None, ..grant(&client, &user, now) };
    s.insert_grant(&appless).await.unwrap();
    assert_eq!(s.find_active_grant(&client, &user, "org-1", None, Surface::Ai).await.unwrap().map(|f| f.id), Some(appless.id));
}

async fn claim_grant_keeps_one_live_grant(db: &dyn DatabaseStore) {
    let s = store(db);
    let now = stored_now();
    let (client, user) = (uid("lgc"), uid("user"));
    let first = grant(&client, &user, now);
    assert_eq!(s.claim_grant(&first).await.unwrap(), first, "the first claim inserts");

    // Signing in again: the same grant, holding the new scopes.
    let again = Grant { scopes: vec!["ai:read".into()], ..grant(&client, &user, now) };
    let claimed = s.claim_grant(&again).await.unwrap();
    assert_eq!(claimed.id, first.id);
    assert_eq!(claimed.scopes, vec!["ai:read".to_string()]);
    assert_eq!(s.get_grant(&first.id).await.unwrap().unwrap().scopes, vec!["ai:read".to_string()]);
    assert!(s.get_grant(&again.id).await.unwrap().is_none(), "no second grant");

    // The database itself refuses a second live grant for the same client,
    // person, org, app and endpoint.
    assert!(s.insert_grant(&grant(&client, &user, now)).await.is_err());

    // Another app is another grant, and so is one after a revocation.
    let appless = Grant { app_id: None, ..grant(&client, &user, now) };
    assert_eq!(s.claim_grant(&appless).await.unwrap().id, appless.id);
    s.revoke_grant(&first.id, now).await.unwrap();
    let fresh = grant(&client, &user, now);
    assert_eq!(s.claim_grant(&fresh).await.unwrap().id, fresh.id);
}

async fn concurrent_claims_share_one_grant(db: &dyn DatabaseStore) {
    let s = store(db);
    let now = stored_now();
    for _ in 0..10 {
        let (client, user) = (uid("lgc"), uid("user"));
        let (a, b) = (grant(&client, &user, now), grant(&client, &user, now));
        let (a, b) = tokio::join!(s.claim_grant(&a), s.claim_grant(&b));
        assert_eq!(a.unwrap().id, b.unwrap().id, "two first sign-ins at once end with one grant");
    }
}

async fn assertion_ids_are_single_use(db: &dyn DatabaseStore) {
    let s = store(db);
    let now = stored_now();
    let (client, jti) = (uid("https://c.example/client.json"), uid("jti"));
    assert!(s.record_assertion(&client, &jti, now + Duration::minutes(5)).await.unwrap(), "first use");
    assert!(!s.record_assertion(&client, &jti, now + Duration::minutes(5)).await.unwrap(), "a replay");
    assert!(s.record_assertion(&uid("https://d.example/client.json"), &jti, now).await.unwrap(), "ids are per client");
}

async fn purge_drops_only_what_expired(db: &dyn DatabaseStore) {
    let s = store(db);
    // Ten years back, so purging never touches rows other tests write
    // concurrently into a shared Postgres database.
    let base = stored_now() - Duration::days(3650);
    let (org, app) = org_and_app(db).await;
    let old = request(&uid("lgc"), base - Duration::days(3));
    let fresh = request(&uid("lgc"), base);
    s.insert_request(&old).await.unwrap();
    s.insert_request(&fresh).await.unwrap();

    let g = grant_in(&org, &app, &uid("lgc"), &uid("user"), base - Duration::days(3));
    s.insert_grant(&g).await.unwrap();
    let stale = token(&g.id, TokenKind::Access, base - Duration::days(3));
    let live = TokenRow { expires_at: base + Duration::days(30), ..token(&g.id, TokenKind::Refresh, base) };
    s.insert_token(&stale).await.unwrap();
    s.insert_token(&live).await.unwrap();

    // (No count of what went: another test's purge may take these rows first.)
    s.purge_expired(base).await.unwrap();
    assert!(s.get_request(&old.id).await.unwrap().is_none());
    assert!(s.get_request(&fresh.id).await.unwrap().is_some());
    assert!(s.get_token(&stale.token_hash).await.unwrap().is_none());
    assert!(s.get_token(&live.token_hash).await.unwrap().is_some());
    assert!(s.get_grant(&g.id).await.unwrap().is_some(), "an active grant stays");

    // A grant revoked over a day ago goes, with its tokens; one revoked
    // since stays.
    s.revoke_grant(&g.id, base - Duration::days(2)).await.unwrap();
    let recent = grant_in(&org, &app, &uid("lgc"), &uid("user"), base);
    s.insert_grant(&recent).await.unwrap();
    s.revoke_grant(&recent.id, base).await.unwrap();
    s.purge_expired(base).await.unwrap();
    assert!(s.get_token(&live.token_hash).await.unwrap().is_none());
    assert!(s.get_grant(&g.id).await.unwrap().is_none());
    assert!(s.get_grant(&recent.id).await.unwrap().is_some());

    // An assertion id is remembered until a day after its assertion expires.
    let (spent, current) = (uid("jti"), uid("jti"));
    s.record_assertion("client", &spent, base - Duration::days(2)).await.unwrap();
    s.record_assertion("client", &current, base).await.unwrap();
    s.purge_expired(base).await.unwrap();
    assert!(s.record_assertion("client", &spent, base).await.unwrap(), "forgotten");
    assert!(!s.record_assertion("client", &current, base).await.unwrap(), "still remembered");
}

async fn purge_drops_unused_clients(db: &dyn DatabaseStore) {
    let s = store(db);
    let base = stored_now() - Duration::days(3650);
    let (org, app) = org_and_app(db).await;

    // Registered but never signed in: gone after 30 days, unless a sign-in
    // is under way.
    let abandoned = client(Registration::Dcr, base - Duration::days(31));
    let newer = client(Registration::Dcr, base - Duration::days(29));
    let signing_in = client(Registration::Dcr, base - Duration::days(31));
    // Signed in before: kept while someone has it connected, and for a year
    // after its last use.
    let used = |days_ago: i64| OAuthClient { last_used_at: Some(base - Duration::days(days_ago)), ..client(Registration::Dcr, base - Duration::days(400)) };
    let (idle, idle_connected, recently_used) = (used(366), used(366), used(300));
    // A copy of a metadata document is fetched again when needed: it goes
    // once it is a day stale and nothing refers to it.
    let stale_doc = client(Registration::Cimd, base - Duration::days(2));
    let stale_doc_connected = client(Registration::Cimd, base - Duration::days(2));
    let fresh_doc = client(Registration::Cimd, base - Duration::hours(12));
    // What refers to a client goes in first: another test's purge may run
    // at any moment against a shared Postgres database.
    s.insert_request(&request(&signing_in.client_id, base)).await.unwrap();
    s.insert_grant(&grant_in(&org, &app, &idle_connected.client_id, &uid("user"), base - Duration::days(400))).await.unwrap();
    s.insert_grant(&grant_in(&org, &app, &stale_doc_connected.client_id, &uid("user"), base - Duration::days(2))).await.unwrap();
    for c in [&abandoned, &newer, &signing_in, &idle, &idle_connected, &recently_used, &stale_doc, &stale_doc_connected, &fresh_doc] {
        s.upsert_client(c).await.unwrap();
    }

    s.purge_expired(base).await.unwrap();
    let kept = |c: &OAuthClient| {
        let id = c.client_id.clone();
        async move { s.get_client(&id).await.unwrap().is_some() }
    };
    assert!(!kept(&abandoned).await, "never signed in, registered 31 days ago");
    assert!(kept(&newer).await);
    assert!(kept(&signing_in).await, "a sign-in is under way");
    assert!(!kept(&idle).await, "no grant, unused for over a year");
    assert!(kept(&idle_connected).await, "someone still has it connected");
    assert!(kept(&recently_used).await);
    assert!(!kept(&stale_doc).await, "a stale copy nothing refers to");
    assert!(kept(&stale_doc_connected).await);
    assert!(kept(&fresh_doc).await);
}

async fn purge_drops_grants_of_deleted_orgs_and_apps(db: &dyn DatabaseStore) {
    let s = store(db);
    let base = stored_now() - Duration::days(3650);
    let (org, app) = org_and_app(db).await;
    let (gone_org, its_app) = org_and_app(db).await;
    let (kept_org, gone_app) = org_and_app(db).await;
    let at = base - Duration::days(2);
    let live = grant_in(&org, &app, &uid("lgc"), &uid("user"), at);
    let appless = Grant { app_id: None, ..grant_in(&org, &app, &uid("lgc"), &uid("user"), at) };
    let org_deleted = grant_in(&gone_org, &its_app, &uid("lgc"), &uid("user"), at);
    let app_deleted = grant_in(&kept_org, &gone_app, &uid("lgc"), &uid("user"), at);
    for g in [&live, &appless, &org_deleted, &app_deleted] {
        s.insert_grant(g).await.unwrap();
    }
    let orphaned_token = token(&org_deleted.id, TokenKind::Refresh, base);
    s.insert_token(&orphaned_token).await.unwrap();
    assert!(db.delete_organization(&gone_org).await.unwrap());
    assert!(db.delete_application(&gone_app).await.unwrap());

    s.purge_expired(base).await.unwrap();
    assert!(s.get_grant(&live.id).await.unwrap().is_some());
    assert!(s.get_grant(&appless.id).await.unwrap().is_some(), "acts in its org's first app");
    assert!(s.get_grant(&org_deleted.id).await.unwrap().is_none(), "its org was deleted");
    assert!(s.get_token(&orphaned_token.token_hash).await.unwrap().is_none(), "with its tokens");
    assert!(s.get_grant(&app_deleted.id).await.unwrap().is_none(), "its app was deleted");
}

async fn purge_drops_tokens_whose_grant_is_gone(db: &dyn DatabaseStore) {
    let s = store(db);
    let base = stored_now() - Duration::days(3650);
    // What another purge leaves when it removes an orphaned grant between
    // this purge's token step and its grant step: a token with no grant.
    // Nothing finds it through the grant again, so it outlived every purge
    // (2026-11-07, purge_drops_grants_of_deleted_orgs_and_apps on a shared
    // Postgres). A token row needs no grant row to exist.
    let stranded = token(&uid("gone-grant"), TokenKind::Refresh, base);
    s.insert_token(&stranded).await.unwrap();
    s.purge_expired(base).await.unwrap();
    assert!(s.get_token(&stranded.token_hash).await.unwrap().is_none(), "a token whose grant is gone");
}

/// Each contract as one test per backend, so a failure names both.
macro_rules! on_every_backend {
    ($($name:ident),* $(,)?) => {
        mod sqlite {
            $(
                #[tokio::test]
                async fn $name() {
                    let db = crate::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap();
                    super::$name(&db).await;
                }
            )*
        }
        mod postgres {
            $(
                #[tokio::test]
                async fn $name() {
                    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
                        eprintln!("skipping: LITEGEN_PG_TEST_URL not set");
                        return;
                    };
                    let db = crate::db::PostgresDatabase::connect(&url).await.expect("connect to LITEGEN_PG_TEST_URL");
                    super::$name(&db).await;
                }
            )*
        }
    };
}

on_every_backend!(
    clients_round_trip,
    request_approve_then_consume_once,
    decide_is_single_shot,
    rotate_refresh_detects_reuse,
    revoke_grant_revokes_tokens,
    find_active_grant_ignores_revoked,
    claim_grant_keeps_one_live_grant,
    concurrent_claims_share_one_grant,
    assertion_ids_are_single_use,
    purge_drops_only_what_expired,
    purge_drops_unused_clients,
    purge_drops_grants_of_deleted_orgs_and_apps,
    purge_drops_tokens_whose_grant_is_gone,
);
