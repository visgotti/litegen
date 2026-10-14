//! [`ManagementStore`] contract tests, run against every backend: SQLite
//! always, Postgres when `LITEGEN_PG_TEST_URL` is set (like `db::postgres_tests`).
//! Ids are unique per run so a shared Postgres database can be reused.

use chrono::{Duration, Utc};
use uuid::Uuid;

use super::types::*;
use crate::db::DatabaseStore;

fn uid(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4().simple())
}

fn limits(t: EntityType, id: &str, org: &str) -> EntityLimits {
    let now = Utc::now();
    EntityLimits {
        entity_type: t,
        entity_id: id.to_string(),
        org_id: Some(org.to_string()),
        max_budget_usd: Some(10.0),
        soft_budget_usd: Some(8.0),
        budget_duration: Some("1d".into()),
        spend_usd: 0.0,
        budget_reset_at: Some(now + Duration::days(1)),
        soft_budget_alerted_at: None,
        allowed_models: Some(vec!["mock/image-gen".into()]),
        max_parallel_requests: Some(3),
        rpm_limit: Some(60),
        metadata: Some(serde_json::json!({ "team": "a" })),
        created_at: now,
        updated_at: now,
    }
}

async fn seed_org(db: &dyn DatabaseStore) -> String {
    let now = Utc::now();
    let id = uid("org");
    db.create_organization(&crate::types::Organization {
        id: id.clone(),
        name: "Store Test".into(),
        slug: id.clone(),
        plan: "free".into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    })
    .await
    .unwrap();
    id
}

async fn limits_contract(db: &dyn DatabaseStore) {
    let store = db.management().expect("backend has a management store");
    let org = seed_org(db).await;
    let key = uid("key");
    let row = limits(EntityType::Key, &key, &org);

    assert!(store.get_limits(EntityType::Key, &key).await.unwrap().is_none());
    store.upsert_limits(&row).await.unwrap();
    let got = store.get_limits(EntityType::Key, &key).await.unwrap().unwrap();
    assert_eq!(got.allowed_models, row.allowed_models);
    assert_eq!(got.metadata, row.metadata);
    assert_eq!((got.max_parallel_requests, got.rpm_limit), (Some(3), Some(60)));
    assert_eq!(got.budget_duration.as_deref(), Some("1d"));
    assert!((got.budget_reset_at.unwrap() - row.budget_reset_at.unwrap()).num_seconds().abs() < 1);

    // Reservation honours the cap atomically.
    assert!(store.reserve_limits_spend(EntityType::Key, &key, 6.0).await.unwrap());
    assert!(!store.reserve_limits_spend(EntityType::Key, &key, 6.0).await.unwrap(), "would pass max_budget_usd");
    assert!(store.reserve_limits_spend(EntityType::Key, &key, 4.0).await.unwrap(), "exactly at the cap is allowed");
    assert!(!store.reserve_limits_spend(EntityType::Key, &uid("nope"), 1.0).await.unwrap(), "missing row");
    assert_eq!(store.charge_limits_spend(EntityType::Key, &key, -2.5).await.unwrap(), Some(7.5));
    assert_eq!(store.charge_limits_spend(EntityType::Key, &uid("nope"), 1.0).await.unwrap(), None);

    // Soft alert is recorded once per window.
    let now = Utc::now();
    assert!(store.mark_soft_budget_alerted(EntityType::Key, &key, now).await.unwrap());
    assert!(!store.mark_soft_budget_alerted(EntityType::Key, &key, now).await.unwrap());

    // Window roll is conditional on the expected reset time.
    let current = store.get_limits(EntityType::Key, &key).await.unwrap().unwrap();
    let next = Some(now + Duration::days(2));
    assert!(!store.roll_limits_window(EntityType::Key, &key, Some(now - Duration::days(9)), next).await.unwrap());
    assert!(store.roll_limits_window(EntityType::Key, &key, current.budget_reset_at, next).await.unwrap());
    let rolled = store.get_limits(EntityType::Key, &key).await.unwrap().unwrap();
    assert_eq!(rolled.spend_usd, 0.0);
    assert!(rolled.soft_budget_alerted_at.is_none());
    assert!(!store.roll_limits_window(EntityType::Key, &key, current.budget_reset_at, next).await.unwrap(), "second roller loses");

    // A lifetime budget (NULL reset time) still rolls NULL-safely.
    let life = uid("app");
    let mut lifetime = limits(EntityType::App, &life, &org);
    lifetime.budget_duration = None;
    lifetime.budget_reset_at = None;
    store.upsert_limits(&lifetime).await.unwrap();
    assert!(store.roll_limits_window(EntityType::App, &life, None, None).await.unwrap());

    // Unlimited rows still track spend.
    let open = uid("member");
    let mut uncapped = limits(EntityType::Member, &open, &org);
    uncapped.max_budget_usd = None;
    store.upsert_limits(&uncapped).await.unwrap();
    assert!(store.reserve_limits_spend(EntityType::Member, &open, 1_000.0).await.unwrap());

    // Listing, by org and type.
    let all = store.list_limits(Some(&org), None).await.unwrap();
    assert_eq!(all.len(), 3);
    let apps = store.list_limits(Some(&org), Some(EntityType::App)).await.unwrap();
    assert_eq!(apps.len(), 1);
    assert!(store.list_limits(None, None).await.unwrap().len() >= 3);

    // Upsert replaces in place.
    let mut edited = store.get_limits(EntityType::Key, &key).await.unwrap().unwrap();
    edited.max_budget_usd = None;
    edited.allowed_models = None;
    store.upsert_limits(&edited).await.unwrap();
    let got = store.get_limits(EntityType::Key, &key).await.unwrap().unwrap();
    assert_eq!((got.max_budget_usd, got.allowed_models), (None, None));

    assert!(store.delete_limits(EntityType::Key, &key).await.unwrap());
    assert!(!store.delete_limits(EntityType::Key, &key).await.unwrap());
}

async fn end_user_contract(db: &dyn DatabaseStore) {
    let store = db.management().unwrap();
    let org = seed_org(db).await;
    let now = Utc::now();
    store.touch_end_user(&org, "u1", now).await.unwrap();
    let u = store.get_end_user(&org, "u1").await.unwrap().unwrap();
    assert!(!u.blocked);
    assert!(u.last_seen_at.is_some());

    let mut edited = u.clone();
    edited.alias = Some("Alias".into());
    edited.blocked = true;
    edited.metadata = Some(serde_json::json!({ "tier": 2 }));
    store.upsert_end_user(&edited).await.unwrap();
    // A later touch only bumps last_seen_at.
    store.touch_end_user(&org, "u1", now + Duration::minutes(5)).await.unwrap();
    let got = store.get_end_user(&org, "u1").await.unwrap().unwrap();
    assert_eq!((got.alias.as_deref(), got.blocked), (Some("Alias"), true));
    assert_eq!(got.metadata, edited.metadata);
    assert!(got.last_seen_at.unwrap() > u.last_seen_at.unwrap());

    store.touch_end_user(&org, "u0", now).await.unwrap();
    let ids: Vec<String> = store.list_end_users(&org).await.unwrap().into_iter().map(|u| u.id).collect();
    assert_eq!(ids, vec!["u0", "u1"]);
    assert!(store.get_end_user(&seed_org(db).await, "u1").await.unwrap().is_none(), "scoped per org");
    assert!(store.delete_end_user(&org, "u1").await.unwrap());
    assert!(!store.delete_end_user(&org, "u1").await.unwrap());
}

async fn spend_contract(db: &dyn DatabaseStore) {
    let store = db.management().unwrap();
    let org = seed_org(db).await;
    let rows = [
        ("k1", Some("eu-a"), "mock/a", "completed", 1.0, vec!["x", "y"]),
        ("k1", Some("eu-b"), "mock/a", "completed", 2.0, vec!["y"]),
        ("k2", None, "mock/b", "failed", 0.0, vec![]),
    ];
    let mut ids = Vec::new();
    for (key, eu, model, status, cost, tags) in rows {
        let id = uid("req");
        db.log_request(&id, model, "mock", status, "image", cost, 10, None, None, Some(&org), None).await.unwrap();
        store
            .attribute_request_log(&id, &Attribution {
                key_id: Some(key.into()),
                user_id: Some("user-1".into()),
                end_user: eu.map(str::to_string),
                tags: tags.into_iter().map(str::to_string).collect(),
            })
            .await
            .unwrap();
        ids.push(id);
    }
    // Attribution of an unknown id is a no-op; re-tagging is idempotent.
    store.attribute_request_log("missing", &Attribution { tags: vec!["z".into()], ..Default::default() }).await.unwrap();
    store
        .attribute_request_log(&ids[0], &Attribution { key_id: Some("k1".into()), user_id: Some("user-1".into()), end_user: Some("eu-a".into()), tags: vec!["x".into()] })
        .await
        .unwrap();

    let f = SpendFilter { org_id: Some(org.clone()), ..Default::default() };
    let by_key = store.spend_report(&f, SpendGroupBy::Key).await.unwrap();
    assert_eq!(by_key[0], SpendReportRow { group: Some("k1".into()), requests: 2, failed_requests: 0, cost_usd: 3.0 });
    assert_eq!(by_key[1], SpendReportRow { group: Some("k2".into()), requests: 1, failed_requests: 1, cost_usd: 0.0 });
    let by_tag = store.spend_report(&f, SpendGroupBy::Tag).await.unwrap();
    assert_eq!(by_tag[0], SpendReportRow { group: Some("y".into()), requests: 2, failed_requests: 0, cost_usd: 3.0 });
    let by_day = store.spend_report(&f, SpendGroupBy::Day).await.unwrap();
    assert_eq!(by_day.len(), 1);
    assert_eq!(by_day[0].group.as_deref(), Some(Utc::now().format("%Y-%m-%d").to_string().as_str()));
    let eu = store.spend_report(&SpendFilter { end_user: Some("eu-b".into()), ..f.clone() }, SpendGroupBy::Model).await.unwrap();
    assert_eq!(eu, vec![SpendReportRow { group: Some("mock/a".into()), requests: 1, failed_requests: 0, cost_usd: 2.0 }]);
    let tagged = store.spend_report(&SpendFilter { tag: Some("x".into()), ..f.clone() }, SpendGroupBy::Status).await.unwrap();
    assert_eq!(tagged.iter().map(|r| r.requests).sum::<u64>(), 1);
    let future = store
        .spend_report(&SpendFilter { from: Some(Utc::now() + Duration::days(1)), ..f.clone() }, SpendGroupBy::Model)
        .await
        .unwrap();
    assert!(future.is_empty());
    let past = store
        .spend_report(&SpendFilter { from: Some(Utc::now() - Duration::days(1)), to: Some(Utc::now() + Duration::days(1)), ..f.clone() }, SpendGroupBy::Model)
        .await
        .unwrap();
    assert_eq!(past.iter().map(|r| r.requests).sum::<u64>(), 3);

    let (page, total) = store.spend_logs(&f, 1, 2).await.unwrap();
    assert_eq!((page.len(), total), (2, 3));
    let (all, _) = store.spend_logs(&f, 1, 10).await.unwrap();
    let first = all.iter().find(|r| r.id == ids[0]).unwrap();
    assert_eq!(first.tags, vec!["x", "y"]);
    assert_eq!(first.end_user.as_deref(), Some("eu-a"));
    assert_eq!(first.user_id.as_deref(), Some("user-1"));
    let (k2, n) = store.spend_logs(&SpendFilter { key_id: Some("k2".into()), ..f.clone() }, 1, 10).await.unwrap();
    assert_eq!((n, k2[0].status.as_str(), k2[0].tags.len()), (1, "failed", 0));
}

async fn config_contract(db: &dyn DatabaseStore) {
    let store = db.management().unwrap();
    let id = format!("mock/{}", uid("rt"));
    store.upsert_runtime_model(&id, "{\"a\":1}", Some("admin")).await.unwrap();
    store.upsert_runtime_model(&id, "{\"a\":2}", None).await.unwrap();
    let models = store.list_runtime_models().await.unwrap();
    let m = models.iter().find(|m| m.id == id).unwrap();
    assert_eq!((m.schema_json.as_str(), m.updated_by.as_deref()), ("{\"a\":2}", None));
    assert!(store.delete_runtime_model(&id).await.unwrap());
    assert!(!store.delete_runtime_model(&id).await.unwrap());

    store.set_model_state(&id, true).await.unwrap();
    assert!(store.list_model_states().await.unwrap().contains(&(id.clone(), true)));
    store.set_model_state(&id, false).await.unwrap();
    assert!(store.list_model_states().await.unwrap().contains(&(id.clone(), false)));

    let name = uid("prov");
    let now = Utc::now();
    let cfg = StoredProviderConfig {
        provider: name.clone(),
        ciphertext: Some("ct".into()),
        nonce: Some("n".into()),
        display_hint: Some("…1234".into()),
        settings_json: "{\"api_base\":\"https://x\"}".into(),
        created_at: now,
        updated_at: now,
    };
    store.upsert_provider_config(&cfg).await.unwrap();
    store.upsert_provider_config(&StoredProviderConfig { ciphertext: None, nonce: None, display_hint: None, ..cfg.clone() }).await.unwrap();
    let got = store.get_provider_config(&name).await.unwrap().unwrap();
    assert_eq!((got.ciphertext, got.settings_json.as_str()), (None, "{\"api_base\":\"https://x\"}"));
    assert!(store.list_provider_configs().await.unwrap().iter().any(|c| c.provider == name));
    assert!(store.delete_provider_config(&name).await.unwrap());
    assert!(store.get_provider_config(&name).await.unwrap().is_none());

    let key = uid("setting");
    assert!(store.get_setting(&key).await.unwrap().is_none());
    store.set_setting(&key, "1").await.unwrap();
    store.set_setting(&key, "2").await.unwrap();
    assert_eq!(store.get_setting(&key).await.unwrap().as_deref(), Some("2"));
    assert!(store.delete_setting(&key).await.unwrap());
    assert!(!store.delete_setting(&key).await.unwrap());
}

async fn run_all(db: &dyn DatabaseStore) {
    limits_contract(db).await;
    end_user_contract(db).await;
    spend_contract(db).await;
    config_contract(db).await;
}

#[tokio::test]
async fn sqlite_management_store() {
    let db = crate::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap();
    run_all(&db).await;
}

#[tokio::test]
async fn postgres_management_store() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping postgres_management_store: LITEGEN_PG_TEST_URL not set");
        return;
    };
    let db = crate::db::PostgresDatabase::connect(&url).await.expect("connect to LITEGEN_PG_TEST_URL");
    run_all(&db).await;
}
