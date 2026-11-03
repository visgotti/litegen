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

async fn clients_round_trip(db: &dyn DatabaseStore) {
    let s = store(db);
    let now = Utc::now();
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
    };
    assert!(s.get_client(&client.client_id).await.unwrap().is_none());
    s.upsert_client(&client).await.unwrap();
    let got = s.get_client(&client.client_id).await.unwrap().expect("stored");
    assert_eq!(got.redirect_uris, client.redirect_uris);
    assert_eq!(got.registration, Registration::Dcr);

    client.client_name = "Renamed".into();
    client.refreshed_at = now + Duration::hours(1);
    s.upsert_client(&client).await.unwrap();
    let got = s.get_client(&client.client_id).await.unwrap().unwrap();
    assert_eq!(got.client_name, "Renamed");
    assert_eq!(got.created_at.timestamp(), now.timestamp(), "an update keeps the first registration time");
}

async fn request_approve_then_consume_once(db: &dyn DatabaseStore) {
    let s = store(db);
    let now = Utc::now();
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
    let now = Utc::now();
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
    let now = Utc::now();
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
    let now = Utc::now();
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
    let now = Utc::now();
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

async fn purge_drops_only_what_expired(db: &dyn DatabaseStore) {
    let s = store(db);
    // Ten years back, so purging never touches rows other tests write
    // concurrently into a shared Postgres database.
    let base = Utc::now() - Duration::days(3650);
    let old = request(&uid("lgc"), base - Duration::days(3));
    let fresh = request(&uid("lgc"), base);
    s.insert_request(&old).await.unwrap();
    s.insert_request(&fresh).await.unwrap();

    let g = grant(&uid("lgc"), &uid("user"), base - Duration::days(3));
    s.insert_grant(&g).await.unwrap();
    let stale = token(&g.id, TokenKind::Access, base - Duration::days(3));
    let live = TokenRow { expires_at: base + Duration::days(30), ..token(&g.id, TokenKind::Refresh, base) };
    s.insert_token(&stale).await.unwrap();
    s.insert_token(&live).await.unwrap();

    let removed = s.purge_expired(base - Duration::days(1)).await.unwrap();
    assert!(removed >= 2, "the old request and the stale token: {removed}");
    assert!(s.get_request(&old.id).await.unwrap().is_none());
    assert!(s.get_request(&fresh.id).await.unwrap().is_some());
    assert!(s.get_token(&stale.token_hash).await.unwrap().is_none());
    assert!(s.get_token(&live.token_hash).await.unwrap().is_some());
    assert!(s.get_grant(&g.id).await.unwrap().is_some(), "an active grant stays");

    // A grant revoked before the cutoff goes, with its tokens; one revoked
    // after it stays.
    s.revoke_grant(&g.id, base - Duration::days(2)).await.unwrap();
    let recent = grant(&uid("lgc"), &uid("user"), base);
    s.insert_grant(&recent).await.unwrap();
    s.revoke_grant(&recent.id, base).await.unwrap();
    s.purge_expired(base - Duration::days(1)).await.unwrap();
    assert!(s.get_token(&live.token_hash).await.unwrap().is_none());
    assert!(s.get_grant(&g.id).await.unwrap().is_none());
    assert!(s.get_grant(&recent.id).await.unwrap().is_some());
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
    purge_drops_only_what_expired,
);
