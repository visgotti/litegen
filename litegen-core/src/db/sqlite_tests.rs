#[cfg(test)]
mod tests {
    use uuid::Uuid;
    use crate::db::sqlite::SqliteDatabase;
    use crate::db::trait_def::DatabaseStore;
    use crate::types::{AuditLogEntry, AuditLogFilter, GenerationStatus, Invitation, PasswordReset, RequestArtifact, Role, Session, UpdateApiKeyRequest, User};
    use crate::db::sqlite::compute_percentiles;
    use crate::auth::tokens::{generate_csrf_token, generate_session_token, token_hash};

    async fn in_memory_db() -> SqliteDatabase {
        SqliteDatabase::connect("sqlite::memory:").await.expect("in-memory sqlite")
    }

    #[tokio::test]
    async fn create_and_get_api_key() {
        let db = in_memory_db().await;
        let key = db.create_api_key("test-key", "hash1", "lg-abc", None, None, "generate,read", None).await.unwrap();
        assert_eq!(key.name, "test-key");
        assert_eq!(key.scopes, "generate,read");
        assert!(key.token_quota.is_none());
        assert_eq!(key.tokens_used, 0.0);

        let fetched = db.get_api_key(&key.id).await.unwrap();
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().id, key.id);
    }

    #[tokio::test]
    async fn create_key_with_quota_and_rpm() {
        let db = in_memory_db().await;
        let key = db.create_api_key("quota-key", "hash2", "lg-qk", Some(10.0), Some(60), "generate", None).await.unwrap();
        assert_eq!(key.token_quota, Some(10.0));
        assert_eq!(key.rpm_limit, Some(60));
        assert_eq!(key.scopes, "generate");
    }

    #[tokio::test]
    async fn list_api_keys_returns_created_keys() {
        let db = in_memory_db().await;
        db.create_api_key("k1", "h1", "lg-1", None, None, "generate,read", None).await.unwrap();
        db.create_api_key("k2", "h2", "lg-2", Some(10.0), Some(60), "generate,read,admin", None).await.unwrap();
        let keys = db.list_api_keys().await.unwrap();
        assert_eq!(keys.len(), 2);
    }

    #[tokio::test]
    async fn update_api_key_changes_quota_and_scopes() {
        let db = in_memory_db().await;
        let key = db.create_api_key("upd", "hash_upd", "lg-upd", None, None, "generate,read", None).await.unwrap();

        let req = UpdateApiKeyRequest {
            name: None,
            token_quota: Some(Some(5.0)),
            rpm_limit: Some(Some(30)),
            scopes: Some("admin".to_string()),
            webhook_url: None,
            expires_at: None,
            is_active: None,
        };
        let updated = db.update_api_key(&key.id, &req).await.unwrap().unwrap();
        assert_eq!(updated.token_quota, Some(5.0));
        assert_eq!(updated.rpm_limit, Some(30));
        assert_eq!(updated.scopes, "admin");
    }

    #[tokio::test]
    async fn update_nonexistent_key_returns_none() {
        let db = in_memory_db().await;
        let fake_id = Uuid::new_v4();
        let req = UpdateApiKeyRequest {
            name: Some("new-name".to_string()),
            token_quota: None,
            rpm_limit: None,
            scopes: None,
            webhook_url: None,
            expires_at: None,
            is_active: None,
        };
        let result = db.update_api_key(&fake_id, &req).await.unwrap();
        assert!(result.is_none());
    }

    /// Review 2026-11-11 P14.
    #[tokio::test]
    async fn a_key_lookup_reports_its_owners_standing_as_the_three_lookups_did() {
        let db = in_memory_db().await;
        let pool = db.pool().clone();
        crate::db::trait_def::one_statement_reads_answer_as_the_lookups(&db, "sqlite", async |key_id: &str| {
            sqlx::query("UPDATE api_keys SET org_id = NULL WHERE id = ?").bind(key_id).execute(&pool).await.unwrap();
        })
        .await;
    }

    /// Sessions are kept by their token's hash (migration 044).
    #[tokio::test]
    async fn sessions_are_kept_by_their_tokens_hash() {
        let db = in_memory_db().await;
        let pool = db.pool().clone();
        crate::db::trait_def::sessions_are_kept_by_their_tokens_hash(&db, "sqlite", async |user_id: &str| {
            let mut ids: Vec<String> = sqlx::query_scalar("SELECT id FROM sessions WHERE user_id = ?").bind(user_id).fetch_all(&pool).await.unwrap();
            ids.sort();
            ids
        })
        .await;
    }

    /// Migration 044 deletes the sessions stored before it, which were kept
    /// by their plain token (SQLite has no SHA-256 to hash them in place):
    /// everyone signs in again once. The accounts stay.
    #[tokio::test]
    async fn migration_0044_signs_everyone_out_once() {
        use sqlx::sqlite::SqlitePoolOptions;

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite:file:mig0044?mode=memory&cache=shared")
            .await
            .expect("connect fresh sqlite");
        let mut before = sqlx::migrate!("./migrations/sqlite");
        before.migrations =
            std::borrow::Cow::Owned(before.migrations.iter().filter(|m| m.version < 20240101000044).cloned().collect());
        before.run(&pool).await.expect("migrations before 044");
        for stmt in [
            "INSERT INTO users (id, email, role) VALUES ('u-signed-in', 'signed-in@example.com', 'member')",
            "INSERT INTO sessions (id, user_id, expires_at, csrf_token) VALUES ('a-plain-token', 'u-signed-in', datetime('now', '+7 days'), 'csrf')",
        ] {
            sqlx::query(stmt).execute(&pool).await.unwrap();
        }

        sqlx::migrate!("./migrations/sqlite").run(&pool).await.expect("migration 044 applies");
        let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions").fetch_one(&pool).await.unwrap();
        assert_eq!(sessions, 0, "the plain-token sessions are gone");
        let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = 'u-signed-in'").fetch_one(&pool).await.unwrap();
        assert_eq!(users, 1, "the account stays");
    }

    #[tokio::test]
    async fn lookup_by_hash_finds_active_key() {
        let db = in_memory_db().await;
        let key = db.create_api_key("lookup", "unique_hash_abc", "lg-lk", None, None, "generate,read", None).await.unwrap();
        let found = db.lookup_api_key_by_hash("unique_hash_abc").await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, key.id);
    }

    #[tokio::test]
    async fn lookup_by_hash_returns_none_for_missing() {
        let db = in_memory_db().await;
        let found = db.lookup_api_key_by_hash("no_such_hash").await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn lookup_by_hash_returns_none_for_revoked_key() {
        let db = in_memory_db().await;
        let key = db.create_api_key("rev", "rev_hash", "lg-rv", None, None, "generate,read", None).await.unwrap();
        db.revoke_api_key(&key.id).await.unwrap();
        let found = db.lookup_api_key_by_hash("rev_hash").await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn atomic_charge_tokens_accumulates() {
        let db = in_memory_db().await;
        let key = db.create_api_key("charge", "charge_hash", "lg-ch", Some(10.0), None, "generate,read", None).await.unwrap();

        let used1 = db.atomic_charge_tokens(&key.id, 3.0).await.unwrap();
        assert!((used1 - 3.0).abs() < 1e-9, "expected 3.0, got {}", used1);

        let used2 = db.atomic_charge_tokens(&key.id, 4.0).await.unwrap();
        assert!((used2 - 7.0).abs() < 1e-9, "expected 7.0, got {}", used2);
    }

    #[tokio::test]
    async fn atomic_charge_tokens_error_on_nonexistent_key() {
        let db = in_memory_db().await;
        let fake_id = Uuid::new_v4();
        let result = db.atomic_charge_tokens(&fake_id, 1.0).await;
        assert!(result.is_err(), "expected error for nonexistent key");
    }

    #[tokio::test]
    async fn revoke_api_key_deactivates_it() {
        let db = in_memory_db().await;
        let key = db.create_api_key("rev2", "rev2_hash", "lg-r2", None, None, "generate", None).await.unwrap();
        let revoked = db.revoke_api_key(&key.id).await.unwrap();
        assert!(revoked);

        // Should not be found by lookup
        let found = db.lookup_api_key_by_hash("rev2_hash").await.unwrap();
        assert!(found.is_none());
    }

    // ─── Generation CRUD ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn insert_and_get_generation_no_key() {
        let db = in_memory_db().await;
        db.insert_generation(
            "litegen-vid-test-1",
            None,
            "mock/video-gen",
            "mock",
            "video",
            Some("job-xyz"),
            0.05,
            None,
            None,
        ).await.unwrap();

        let gen = db.get_generation("litegen-vid-test-1").await.unwrap();
        assert!(gen.is_some(), "should find inserted generation");
        let gen = gen.unwrap();
        assert_eq!(gen.id, "litegen-vid-test-1");
        assert!(gen.key_id.is_none(), "key_id should be None for master key");
        assert_eq!(gen.model, "mock/video-gen");
        assert_eq!(gen.provider, "mock");
        assert_eq!(gen.status, GenerationStatus::Pending);
        assert_eq!(gen.progress, 0);
        assert_eq!(gen.provider_job_id.as_deref(), Some("job-xyz"));
    }

    #[tokio::test]
    async fn insert_and_get_generation_with_key() {
        let db = in_memory_db().await;
        let key = db.create_api_key("gen-key", "gen-hash", "lg-gk", None, None, "generate,read", None).await.unwrap();

        db.insert_generation(
            "litegen-vid-test-2",
            Some(&key.id),
            "mock/video-gen",
            "mock",
            "video",
            Some("job-abc"),
            0.10,
            None,
            None,
        ).await.unwrap();

        let gen = db.get_generation("litegen-vid-test-2").await.unwrap().unwrap();
        assert_eq!(gen.key_id, Some(key.id), "key_id should match");
        assert!((gen.cost_usd - 0.10).abs() < 1e-9);
    }

    #[tokio::test]
    async fn update_generation_status_to_completed() {
        let db = in_memory_db().await;
        db.insert_generation("litegen-vid-upd-1", None, "mock/video-gen", "mock", "video", Some("j1"), 0.0, None, None).await.unwrap();

        let now = chrono::Utc::now();
        db.update_generation_status(
            "litegen-vid-upd-1",
            "completed",
            100,
            Some("https://example.com/video.mp4"),
            None,
            Some(now),
        ).await.unwrap();

        let gen = db.get_generation("litegen-vid-upd-1").await.unwrap().unwrap();
        assert_eq!(gen.status, GenerationStatus::Completed);
        assert_eq!(gen.progress, 100);
        assert_eq!(gen.result_url.as_deref(), Some("https://example.com/video.mp4"));
        assert!(gen.completed_at.is_some());
    }

    #[tokio::test]
    async fn update_generation_status_to_failed() {
        let db = in_memory_db().await;
        db.insert_generation("litegen-vid-fail-1", None, "mock/video-gen", "mock", "video", Some("j2"), 0.0, None, None).await.unwrap();

        db.update_generation_status(
            "litegen-vid-fail-1",
            "failed",
            0,
            None,
            Some("provider error"),
            Some(chrono::Utc::now()),
        ).await.unwrap();

        let gen = db.get_generation("litegen-vid-fail-1").await.unwrap().unwrap();
        assert_eq!(gen.status, GenerationStatus::Failed);
        assert_eq!(gen.error_message.as_deref(), Some("provider error"));
    }

    #[tokio::test]
    async fn list_active_generations_returns_pending_and_processing() {
        let db = in_memory_db().await;

        db.insert_generation("litegen-vid-act-1", None, "mock/video-gen", "mock", "video", Some("j1"), 0.0, None, None).await.unwrap();
        db.insert_generation("litegen-vid-act-2", None, "mock/video-gen", "mock", "video", Some("j2"), 0.0, None, None).await.unwrap();
        db.insert_generation("litegen-vid-act-3", None, "mock/video-gen", "mock", "video", Some("j3"), 0.0, None, None).await.unwrap();

        // Mark one as completed
        db.update_generation_status("litegen-vid-act-3", "completed", 100, None, None, Some(chrono::Utc::now())).await.unwrap();

        let active = db.list_active_generations(100).await.unwrap();
        assert_eq!(active.len(), 2, "should list 2 active generations (pending/processing)");
        assert!(active.iter().all(|g| matches!(g.status, GenerationStatus::Pending | GenerationStatus::Processing)));
    }

    #[tokio::test]
    async fn get_generation_returns_none_for_unknown_id() {
        let db = in_memory_db().await;
        let result = db.get_generation("litegen-vid-nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    // ─── list_generations / count_generations ─────────────────────────────────

    #[tokio::test]
    async fn list_generations_owned_rows_visible_to_key() {
        let db = in_memory_db().await;
        let key1 = db.create_api_key("k1", "h1", "lg-1", None, None, "generate,read", None).await.unwrap();
        let key2 = db.create_api_key("k2", "h2", "lg-2", None, None, "generate,read", None).await.unwrap();

        // key1 owns gen-1; key2 owns gen-2; gen-3 has no key (master-key row)
        db.insert_generation("lg-gen-1", Some(&key1.id), "mock/v", "mock", "video", None, 0.0, None, None).await.unwrap();
        db.insert_generation("lg-gen-2", Some(&key2.id), "mock/v", "mock", "video", None, 0.0, None, None).await.unwrap();
        db.insert_generation("lg-gen-3", None, "mock/v", "mock", "video", None, 0.0, None, None).await.unwrap();

        // key1 should see gen-1 AND gen-3 (NULL key_id rows are always visible)
        let rows = db.list_generations(Some(&key1.id), 1, 50).await.unwrap();
        let ids: Vec<&str> = rows.iter().map(|g| g.id.as_str()).collect();
        assert!(ids.contains(&"lg-gen-1"), "key1 should see own row");
        assert!(ids.contains(&"lg-gen-3"), "key1 should see master-key rows");
        assert!(!ids.contains(&"lg-gen-2"), "key1 should NOT see key2 rows");

        let count = db.count_generations(Some(&key1.id)).await.unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn list_generations_master_key_sees_all() {
        let db = in_memory_db().await;
        let key1 = db.create_api_key("k1", "h1", "lg-1", None, None, "generate,read", None).await.unwrap();
        db.insert_generation("lg-all-1", Some(&key1.id), "mock/v", "mock", "video", None, 0.0, None, None).await.unwrap();
        db.insert_generation("lg-all-2", None, "mock/v", "mock", "video", None, 0.0, None, None).await.unwrap();

        // master key (None) sees all
        let rows = db.list_generations(None, 1, 50).await.unwrap();
        assert_eq!(rows.len(), 2);
        let count = db.count_generations(None).await.unwrap();
        assert_eq!(count, 2);
    }

    // ─── cancel_generation ────────────────────────────────────────────────────

    #[tokio::test]
    async fn cancel_generation_on_pending_succeeds() {
        let db = in_memory_db().await;
        db.insert_generation("lg-cancel-1", None, "mock/v", "mock", "video", None, 0.0, None, None).await.unwrap();

        let result = db.cancel_generation("lg-cancel-1").await.unwrap();
        assert!(result.is_some(), "cancel should return the updated row");
        let gen = result.unwrap();
        assert_eq!(gen.status, GenerationStatus::Cancelled);
        assert!(gen.completed_at.is_some());
    }

    #[tokio::test]
    async fn cancel_generation_on_completed_returns_none() {
        let db = in_memory_db().await;
        db.insert_generation("lg-cancel-2", None, "mock/v", "mock", "video", None, 0.0, None, None).await.unwrap();
        db.update_generation_status("lg-cancel-2", "completed", 100, None, None, Some(chrono::Utc::now())).await.unwrap();

        let result = db.cancel_generation("lg-cancel-2").await.unwrap();
        assert!(result.is_none(), "cancel on completed should return None (409 condition)");
    }

    // ─── get_request_logs_filtered ────────────────────────────────────────────

    #[tokio::test]
    async fn logs_filtered_by_model() {
        let db = in_memory_db().await;
        db.log_request("id1", "openai/dall-e-3", "openai", "completed", "image", 0.01, 100, None, None, None, None).await.unwrap();
        db.log_request("id2", "mock/image-gen", "mock", "completed", "image", 0.0, 50, None, None, None, None).await.unwrap();
        db.log_request("id3", "mock/image-gen", "mock", "failed", "image", 0.0, 20, Some("err"), None, None, None).await.unwrap();

        let filters = crate::types::LogFilters {
            model: Some("mock/image-gen".to_string()),
            ..Default::default()
        };
        let (logs, total) = db.get_request_logs_filtered(&filters, 1, 50).await.unwrap();
        assert_eq!(total, 2, "should return 2 mock logs");
        assert!(logs.iter().all(|l| l.model == "mock/image-gen"));
    }

    #[tokio::test]
    async fn logs_filtered_by_status() {
        let db = in_memory_db().await;
        db.log_request("s1", "openai/dall-e-3", "openai", "completed", "image", 0.01, 100, None, None, None, None).await.unwrap();
        db.log_request("s2", "mock/image-gen", "mock", "failed", "image", 0.0, 50, Some("err"), None, None, None).await.unwrap();
        db.log_request("s3", "mock/image-gen", "mock", "completed", "image", 0.0, 50, None, None, None, None).await.unwrap();

        let filters = crate::types::LogFilters {
            status: Some("failed".to_string()),
            ..Default::default()
        };
        let (logs, total) = db.get_request_logs_filtered(&filters, 1, 50).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(logs[0].id, "s2");
    }

    // ─── update_request_log_status ──────────────────────────────────────────

    #[tokio::test]
    async fn update_request_log_status_moves_a_pending_row_to_its_terminal_status() {
        let db = in_memory_db().await;
        // 999_999 ms (~16 min) stands in for a submit-time latency no test run
        // can reach, so the re-stamp below is unmistakable without pinning a
        // wall-clock number.
        let started = std::time::Instant::now();
        db.log_request("rl-fail", "mock/video-gen", "mock", "pending", "video", 0.25, 999_999, None, None, None, None).await.unwrap();
        db.log_request("rl-done", "mock/mesh-3d", "mock", "pending", "model3d", 0.0, 30, None, None, None, None).await.unwrap();

        db.update_request_log_status("rl-fail", "failed", Some("provider error")).await.unwrap();
        db.update_request_log_status("rl-done", "completed", None).await.unwrap();

        let (logs, _) = db.get_request_logs(1, 50).await.unwrap();
        let failed = logs.iter().find(|l| l.id == "rl-fail").unwrap();
        assert_eq!(failed.status, GenerationStatus::Failed);
        assert_eq!(failed.error.as_deref(), Some("provider error"));
        let done = logs.iter().find(|l| l.id == "rl-done").unwrap();
        assert_eq!(done.status, GenerationStatus::Completed);
        assert_eq!(done.error, None);
        // A row that was `pending` is an ASYNC row whose logged latency is the
        // enqueue time; settling it re-stamps `latency_ms` with the END-TO-END
        // time (now − created_at) so `/v1/stats` percentiles keep one meaning.
        // `created_at` is `datetime('now')` — truncated to the second — so the
        // re-stamp can read up to a second high; bound it by the test's own
        // elapsed time plus that second instead of asserting an exact number.
        let ceiling = started.elapsed().as_millis() as u64 + 2_000;
        assert!(
            failed.latency_ms <= ceiling,
            "a settled pending row carries its end-to-end latency, not the 999_999ms submit stamp; got {}",
            failed.latency_ms
        );
        // Everything else about the submit-time row stays as logged.
        assert!((failed.cost_usd - 0.25).abs() < 1e-9);
    }

    #[tokio::test]
    async fn update_request_log_status_leaves_a_non_pending_rows_latency_alone() {
        // The re-stamp is conditional on the PRE-UPDATE status (SQL evaluates
        // SET expressions against the old row). A sync image row already carries
        // the latency its handler measured end to end, so settling it — a cancel,
        // say — must not overwrite that with time-since-created.
        let db = in_memory_db().await;
        db.log_request("rl-sync", "mock/image-gen", "mock", "completed", "image", 0.01, 1234, None, None, None, None).await.unwrap();

        db.update_request_log_status("rl-sync", "cancelled", None).await.unwrap();

        let (logs, _) = db.get_request_logs(1, 50).await.unwrap();
        let row = logs.iter().find(|l| l.id == "rl-sync").unwrap();
        assert_eq!(row.status, GenerationStatus::Cancelled);
        assert_eq!(row.latency_ms, 1234, "a row that was never `pending` keeps its measured latency");
    }

    // ─── Audit Log ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn insert_and_list_audit_log() {
        let db = in_memory_db().await;

        let entry = AuditLogEntry {
            id: format!("audit-{}", Uuid::new_v4()),
            actor_key_id: Some("key-id-1".to_string()),
            actor_label: "test-key".to_string(),
            action: "key.create".to_string(),
            target_type: "api_key".to_string(),
            target_id: "target-key-id".to_string(),
            before_json: None,
            after_json: Some(r#"{"name":"test"}"#.to_string()),
            created_at: chrono::Utc::now(),
            org_id: None,
        };

        db.insert_audit_log(&entry).await.unwrap();

        // List all entries — should have 1.
        let (entries, total) = db
            .list_audit_log(&AuditLogFilter::default(), 1, 50)
            .await
            .unwrap();
        assert_eq!(total, 1);
        assert_eq!(entries[0].action, "key.create");
        assert_eq!(entries[0].target_id, "target-key-id");
    }

    /// Item 11 (2026-11-15): an entry is stored under the org it names, and
    /// one that names none under no org; the server decides which
    /// (`api::audit::audit_org`). The default org was the fallback here.
    #[tokio::test]
    async fn an_audit_entry_without_an_org_is_stored_under_none() {
        let db = in_memory_db().await;
        let entry = |id: &str, org_id: Option<&str>| AuditLogEntry {
            id: id.to_string(),
            actor_key_id: None,
            actor_label: "master-key".to_string(),
            action: "settings.update".to_string(),
            target_type: "settings".to_string(),
            target_id: "gateway".to_string(),
            before_json: None,
            after_json: None,
            created_at: chrono::Utc::now(),
            org_id: org_id.map(str::to_string),
        };
        db.insert_audit_log(&entry("a-none", None)).await.unwrap();
        db.insert_audit_log(&entry("a-default", Some(crate::api::middleware::DEFAULT_ORG_ID))).await.unwrap();

        let (entries, _) = db.list_audit_log(&AuditLogFilter::default(), 1, 50).await.unwrap();
        let org_of = |id: &str| entries.iter().find(|e| e.id == id).expect("stored").org_id.clone();
        assert_eq!(org_of("a-none"), None, "an entry that names no org");
        assert_eq!(org_of("a-default").as_deref(), Some(crate::api::middleware::DEFAULT_ORG_ID));
    }

    /// One audit entry, as the item 11 tests write them.
    fn audit_row(id: &str, actor_key_id: Option<&str>, actor_label: &str, action: &str, target_type: &str, org_id: Option<&str>) -> AuditLogEntry {
        AuditLogEntry {
            id: id.to_string(),
            actor_key_id: actor_key_id.map(str::to_string),
            actor_label: actor_label.to_string(),
            action: action.to_string(),
            target_type: target_type.to_string(),
            target_id: format!("target-of-{id}"),
            before_json: None,
            after_json: None,
            created_at: chrono::Utc::now(),
            org_id: org_id.map(str::to_string),
        }
    }

    /// Item 11: a listing reads one org's entries or those filed under no
    /// org, never both. An org's members read the first; in hosted mode the
    /// master key reads the second.
    #[tokio::test]
    async fn the_audit_log_lists_one_orgs_entries_or_those_filed_under_none() {
        use crate::api::middleware::DEFAULT_ORG_ID;
        use crate::db::{AuditLogOrg, TenantAuditFilter};
        let db = in_memory_db().await;
        for entry in [
            audit_row("a-none", None, "master-key", "settings.update", "settings", None),
            audit_row("a-none-routes", None, "master-key", "routes.update", "routes", None),
            audit_row("a-default", None, "master-key", "settings.update", "settings", Some(DEFAULT_ORG_ID)),
        ] {
            db.insert_audit_log(&entry).await.unwrap();
        }
        let ids = |entries: Vec<AuditLogEntry>| {
            let mut ids: Vec<String> = entries.into_iter().map(|e| e.id).collect();
            ids.sort();
            ids
        };
        let all = TenantAuditFilter::default();

        let (entries, total) = db.list_audit_log_for_tenant_filtered(AuditLogOrg::NoOrg, &all, 1, 50).await.unwrap();
        assert_eq!((ids(entries), total), (vec!["a-none".to_string(), "a-none-routes".to_string()], 2), "filed under no org");
        let (entries, total) = db.list_audit_log_for_tenant_filtered(AuditLogOrg::Org(DEFAULT_ORG_ID), &all, 1, 50).await.unwrap();
        assert_eq!((ids(entries), total), (vec!["a-default".to_string()], 1), "the default org's");
        let routes = TenantAuditFilter { action: Some("routes.update"), ..Default::default() };
        let (entries, total) = db.list_audit_log_for_tenant_filtered(AuditLogOrg::NoOrg, &routes, 1, 50).await.unwrap();
        assert_eq!((ids(entries), total), (vec!["a-none-routes".to_string()], 1), "the filters apply");
    }

    /// Item 11: the hosted start-up step moves exactly the master key's
    /// entries filed under the org for one of the actions, to no org, and a
    /// second run moves none.
    #[tokio::test]
    async fn the_master_keys_entries_for_the_named_actions_move_to_no_org() {
        use crate::api::middleware::DEFAULT_ORG_ID;
        let db = in_memory_db().await;
        let rows = [
            audit_row("m-provider", None, "master-key", "provider.configure", "provider", Some(DEFAULT_ORG_ID)),
            audit_row("m-settings", None, "master-key", "settings.update", "settings", Some(DEFAULT_ORG_ID)),
            audit_row("m-user", None, "master-key", "user.update", "user", Some(DEFAULT_ORG_ID)),
            audit_row("m-org-limits", None, "master-key", "limits.update", "org_limits", Some(DEFAULT_ORG_ID)),
            audit_row("person", None, "owner@deploy.test", "settings.update", "settings", Some(DEFAULT_ORG_ID)),
            audit_row("key", Some("key-1"), "key-1", "settings.update", "settings", Some(DEFAULT_ORG_ID)),
            audit_row("system", None, "system", "budget.soft_limit_reached", "org_limits", Some(DEFAULT_ORG_ID)),
            audit_row("m-other-org", None, "master-key", "settings.update", "settings", Some("org-b")),
            audit_row("m-no-org", None, "master-key", "settings.update", "settings", None),
        ];
        for entry in &rows {
            db.insert_audit_log(entry).await.unwrap();
        }
        let actions = ["provider.configure", "settings.update", "user.update"];

        let moved = db.move_master_key_audit_entries_to_no_org(DEFAULT_ORG_ID, &actions).await.unwrap();
        let (entries, _) = db.list_audit_log(&AuditLogFilter::default(), 1, 50).await.unwrap();
        let org_of = |id: &str| entries.iter().find(|e| e.id == id).expect("stored").org_id.clone();
        for entry in &rows {
            let expected = match entry.id.as_str() {
                "m-provider" | "m-settings" | "m-user" => None,
                _ => entry.org_id.clone(),
            };
            assert_eq!(org_of(&entry.id), expected, "{}", entry.id);
        }
        assert_eq!(moved, 3);
        assert_eq!(db.move_master_key_audit_entries_to_no_org(DEFAULT_ORG_ID, &actions).await.unwrap(), 0, "a second run moves none");
    }

    // ─── Latency percentiles ──────────────────────────────────────────────────

    #[tokio::test]
    async fn latency_percentiles_100_samples() {
        let db = in_memory_db().await;
        // Insert 100 completed requests with latency 1..=100 ms (in arbitrary order).
        for i in 1u64..=100 {
            db.log_request(
                &format!("perc-{}", i),
                "mock/v",
                "mock",
                "completed",
                "image",
                0.0,
                i as i64,
                None,
                None,
                None,
                None,
            ).await.unwrap();
        }

        let p = db.latency_percentiles(60).await.unwrap();

        assert_eq!(p.sample_count, 100, "should have 100 samples");

        // Nearest-rank method: p50 = ceil(50% * 100) = 50th value
        assert_eq!(p.p50_ms as i64, 50, "p50 should be 50ms, got {}ms", p.p50_ms);
        // p95 = ceil(95% * 100) = 95th value
        assert_eq!(p.p95_ms as i64, 95, "p95 should be 95ms, got {}ms", p.p95_ms);
        // p99 = ceil(99% * 100) = 99th value
        assert_eq!(p.p99_ms as i64, 99, "p99 should be 99ms, got {}ms", p.p99_ms);
        assert_eq!(p.window_minutes, 60);
    }

    #[test]
    fn compute_percentiles_empty() {
        let p = compute_percentiles(vec![], 60);
        assert_eq!(p.sample_count, 0);
        assert_eq!(p.p50_ms, 0.0);
        assert_eq!(p.p95_ms, 0.0);
        assert_eq!(p.p99_ms, 0.0);
    }

    #[test]
    fn compute_percentiles_single() {
        let p = compute_percentiles(vec![(42,)], 60);
        assert_eq!(p.sample_count, 1);
        assert_eq!(p.p50_ms, 42.0);
        assert_eq!(p.p95_ms, 42.0);
        assert_eq!(p.p99_ms, 42.0);
    }

    #[tokio::test]
    async fn latency_percentiles_excludes_failed_requests() {
        let db = in_memory_db().await;
        // 5 completed requests with latencies 10,20,30,40,50
        for (id, lat) in [("pf1", 10i64), ("pf2", 20), ("pf3", 30), ("pf4", 40), ("pf5", 50)] {
            db.log_request(id, "mock/v", "mock", "completed", "image", 0.0, lat, None, None, None, None).await.unwrap();
        }
        // 3 failed requests with very high latency that should NOT affect percentiles
        for (id, lat) in [("pf6", 10000i64), ("pf7", 20000), ("pf8", 30000)] {
            db.log_request(id, "mock/v", "mock", "failed", "image", 0.0, lat, Some("err"), None, None, None).await.unwrap();
        }

        let p = db.latency_percentiles(60).await.unwrap();
        assert_eq!(p.sample_count, 5, "failed requests should be excluded");
        assert!(p.p99_ms <= 50.0, "p99 should not include failed request latencies");
    }

    // ─── Request Artifacts ────────────────────────────────────────────────────

    #[tokio::test]
    async fn insert_and_get_request_artifact() {
        let db = in_memory_db().await;

        let artifact = RequestArtifact {
            request_id: "test-req-1".to_string(),
            media_type: "image".to_string(),
            prompt: Some("a test prompt".to_string()),
            negative_prompt: Some("blurry".to_string()),
            params_json: Some(serde_json::json!({"size": "1024x1024", "quality": "hd"})),
            refs_meta_json: None,
            output_kind: "b64".to_string(),
            output_value: Some("iVBORw0KGgo=".to_string()),
            output_mime: Some("image/png".to_string()),
            output_truncated: false,
            error_message: None,
            created_at: chrono::Utc::now(),
            org_id: None,
            app_id: None,
        };

        db.insert_request_artifact(&artifact).await.unwrap();

        let fetched = db.get_request_artifact("test-req-1").await.unwrap();
        assert!(fetched.is_some(), "artifact should exist after insert");
        let fetched = fetched.unwrap();
        assert_eq!(fetched.request_id, "test-req-1");
        assert_eq!(fetched.media_type, "image");
        assert_eq!(fetched.prompt.as_deref(), Some("a test prompt"));
        assert_eq!(fetched.negative_prompt.as_deref(), Some("blurry"));
        assert_eq!(fetched.output_kind, "b64");
        assert_eq!(fetched.output_value.as_deref(), Some("iVBORw0KGgo="));
        assert_eq!(fetched.output_mime.as_deref(), Some("image/png"));
        assert!(!fetched.output_truncated);
        assert!(fetched.params_json.is_some());
        let params = fetched.params_json.unwrap();
        assert_eq!(params["size"], "1024x1024");
    }

    #[tokio::test]
    async fn get_request_artifact_returns_none_for_missing() {
        let db = in_memory_db().await;
        let result = db.get_request_artifact("nonexistent-id").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn insert_artifact_with_truncated_flag() {
        let db = in_memory_db().await;

        let artifact = RequestArtifact {
            request_id: "test-trunc-1".to_string(),
            media_type: "image".to_string(),
            prompt: Some("large image".to_string()),
            negative_prompt: None,
            params_json: None,
            refs_meta_json: None,
            output_kind: "b64".to_string(),
            output_value: Some("truncated_b64_data".to_string()),
            output_mime: Some("image/png".to_string()),
            output_truncated: true,
            error_message: None,
            created_at: chrono::Utc::now(),
            org_id: None,
            app_id: None,
        };

        db.insert_request_artifact(&artifact).await.unwrap();

        let fetched = db.get_request_artifact("test-trunc-1").await.unwrap().unwrap();
        assert!(fetched.output_truncated, "truncated flag should round-trip");
    }

    #[tokio::test]
    async fn insert_error_artifact() {
        let db = in_memory_db().await;

        let artifact = RequestArtifact {
            request_id: "test-err-1".to_string(),
            media_type: "image".to_string(),
            prompt: Some("failing prompt".to_string()),
            negative_prompt: None,
            params_json: None,
            refs_meta_json: None,
            output_kind: "error".to_string(),
            output_value: None,
            output_mime: None,
            output_truncated: false,
            error_message: Some("provider rate limited".to_string()),
            created_at: chrono::Utc::now(),
            org_id: None,
            app_id: None,
        };

        db.insert_request_artifact(&artifact).await.unwrap();

        let fetched = db.get_request_artifact("test-err-1").await.unwrap().unwrap();
        assert_eq!(fetched.output_kind, "error");
        assert_eq!(fetched.error_message.as_deref(), Some("provider rate limited"));
        assert!(fetched.output_value.is_none());
    }

    #[tokio::test]
    async fn audit_log_filter_by_action() {
        let db = in_memory_db().await;

        let make_entry = |id: &str, action: &str| AuditLogEntry {
            id: id.to_string(),
            actor_key_id: None,
            actor_label: "master-key".to_string(),
            action: action.to_string(),
            target_type: "api_key".to_string(),
            target_id: "t1".to_string(),
            before_json: None,
            after_json: None,
            created_at: chrono::Utc::now(),
            org_id: None,
        };

        db.insert_audit_log(&make_entry("a1", "key.create")).await.unwrap();
        db.insert_audit_log(&make_entry("a2", "key.revoke")).await.unwrap();
        db.insert_audit_log(&make_entry("a3", "key.create")).await.unwrap();

        let filter = AuditLogFilter {
            action: Some("key.create".to_string()),
            ..Default::default()
        };
        let (entries, total) = db.list_audit_log(&filter, 1, 50).await.unwrap();
        assert_eq!(total, 2);
        assert!(entries.iter().all(|e| e.action == "key.create"));
    }

    // ─── User / Session / Invitation / PasswordReset / LoginAttempt tests ─────

    fn make_user(email: &str, role: Role) -> User {
        User {
            id: Uuid::new_v4().to_string(),
            email: email.to_string(),
            password_hash: Some("$argon2id$v=19$...fake_hash".to_string()),
            role,
            oauth_github_id: None,
            oauth_google_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_login_at: None,
            is_active: true,
            sign_in_epoch: 0,
        }
    }

    #[tokio::test]
    async fn create_user_then_get_by_email() {
        let db = in_memory_db().await;
        let u = make_user("joe@example.com", Role::Owner);
        db.create_user(&u).await.unwrap();
        let got = db.get_user_by_email("joe@example.com").await.unwrap();
        assert!(got.is_some());
        let got = got.unwrap();
        assert_eq!(got.email, "joe@example.com");
        assert_eq!(got.role, Role::Owner);
        assert!(got.is_active);
    }

    #[tokio::test]
    async fn count_users_zero_then_one() {
        let db = in_memory_db().await;
        assert_eq!(db.count_users().await.unwrap(), 0);
        let u = make_user("count@example.com", Role::Member);
        db.create_user(&u).await.unwrap();
        assert_eq!(db.count_users().await.unwrap(), 1);
    }

    /// The first single-tenant signup makes the account and its Owner
    /// membership together, only while no account exists, also where the
    /// accounts predate the first-owner claim (migration 034).
    #[tokio::test]
    async fn the_first_owner_is_made_once_with_its_membership() {
        use crate::api::middleware::DEFAULT_ORG_ID;
        let db = in_memory_db().await;
        let first = make_user("first@example.com", Role::Owner);
        assert!(db.create_first_owner(&first, DEFAULT_ORG_ID).await.unwrap());
        assert_eq!(db.get_membership(DEFAULT_ORG_ID, &first.id).await.unwrap(), Some(Role::Owner));
        let second = make_user("second@example.com", Role::Owner);
        assert!(!db.create_first_owner(&second, DEFAULT_ORG_ID).await.unwrap(), "signup is closed");
        assert!(db.get_user_by_id(&second.id).await.unwrap().is_none(), "nothing written");

        let older = in_memory_db().await;
        older.create_user(&make_user("old@example.com", Role::Owner)).await.unwrap();
        let late = make_user("late@example.com", Role::Owner);
        assert!(!older.create_first_owner(&late, DEFAULT_ORG_ID).await.unwrap(), "accounts made before the claim close it too");
        assert_eq!(older.count_users().await.unwrap(), 1);
    }

    /// A first owner whose membership can't be written leaves no account,
    /// and takes its claim with it: signup stays open.
    #[tokio::test]
    async fn a_first_owner_whose_membership_fails_leaves_no_account() {
        use crate::api::middleware::DEFAULT_ORG_ID;
        let db = in_memory_db().await;
        let first = make_user("first@example.com", Role::Owner);
        assert!(db.create_first_owner(&first, "no-such-org").await.is_err(), "the membership's org must exist");
        assert_eq!(db.count_users().await.unwrap(), 0, "no account left behind");
        assert!(db.create_first_owner(&first, DEFAULT_ORG_ID).await.unwrap(), "the first signup is still open");
    }

    /// First signups at once, each on a connection of its own, make one
    /// Owner; the others find signup closed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn first_owners_at_once_make_one() {
        use crate::api::middleware::DEFAULT_ORG_ID;
        let db = std::sync::Arc::new(in_memory_db().await);
        let start = std::sync::Arc::new(tokio::sync::Barrier::new(8));
        let tries: Vec<_> = (0..8)
            .map(|i| {
                let (db, start) = (db.clone(), start.clone());
                tokio::spawn(async move {
                    let user = make_user(&format!("first{i}@example.com"), Role::Owner);
                    start.wait().await;
                    db.create_first_owner(&user, DEFAULT_ORG_ID).await
                })
            })
            .collect();
        let mut won = 0;
        for t in tries {
            won += usize::from(t.await.unwrap().expect("a first signup that loses hears signup is closed"));
        }
        assert_eq!(won, 1, "one first Owner");
        assert_eq!(db.count_users().await.unwrap(), 1);
        let owners = db.list_org_members(DEFAULT_ORG_ID).await.unwrap().into_iter().filter(|m| m.role == Role::Owner).count();
        assert_eq!(owners, 1);
    }

    /// An org and its first app, for `create_org_with_owner`; `app_org` is
    /// the org the app names (another one makes the app's write fail).
    fn org_and_app(name: &str, app_org: Option<&str>) -> (crate::types::Organization, crate::types::Application) {
        let now = chrono::Utc::now();
        let org = crate::types::Organization {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            slug: format!("{}-{}", name.to_lowercase(), Uuid::new_v4().simple()),
            plan: "free".into(),
            status: "active".into(),
            created_at: now,
            updated_at: now,
        };
        let app = crate::types::Application {
            id: Uuid::new_v4().to_string(),
            org_id: app_org.map_or_else(|| org.id.clone(), str::to_string),
            name: "Default".into(),
            slug: "default".into(),
            status: "active".into(),
            created_at: now,
            updated_at: now,
        };
        (org, app)
    }

    /// Follow-up item 4: an org, its Owner's membership and its first app
    /// (and, for a signup, the account) are made together, or not at all.
    #[tokio::test]
    async fn an_org_and_its_owner_are_made_together_or_not_at_all() {
        use crate::db::OrgOwner;
        let db = in_memory_db().await;

        // A signup whose app can't be written leaves no account and no org.
        let signer = make_user("signer@example.com", Role::Owner);
        let (org, app) = org_and_app("Signer", Some("no-such-org"));
        assert!(db.create_org_with_owner(&org, &app, OrgOwner::New(&signer)).await.is_err(), "the app's org must exist");
        assert!(db.get_user_by_email("signer@example.com").await.unwrap().is_none(), "no account left behind");
        assert!(db.get_organization(&org.id).await.unwrap().is_none(), "no org left behind");

        // An org whose Owner can't be made a member leaves no org.
        let (org, app) = org_and_app("Orphan", None);
        assert!(db.create_org_with_owner(&org, &app, OrgOwner::Existing("no-such-user")).await.is_err(), "the owner must exist");
        assert!(db.get_organization(&org.id).await.unwrap().is_none(), "no org left behind");

        // Made: the account, the org, the Owner membership, the app.
        let (org, app) = org_and_app("Signer", None);
        db.create_org_with_owner(&org, &app, OrgOwner::New(&signer)).await.unwrap();
        assert!(db.get_user_by_id(&signer.id).await.unwrap().is_some());
        assert_eq!(db.get_membership(&org.id, &signer.id).await.unwrap(), Some(Role::Owner));
        assert_eq!(db.list_apps_for_org(&org.id).await.unwrap().into_iter().map(|a| a.id).collect::<Vec<_>>(), [app.id]);
    }

    #[tokio::test]
    async fn transfer_owner_demotes_old_and_promotes_new() {
        let db = in_memory_db().await;
        let owner = make_user("owner@example.com", Role::Owner);
        let admin = make_user("admin@example.com", Role::Admin);
        db.create_user(&owner).await.unwrap();
        db.create_user(&admin).await.unwrap();
        db.transfer_owner(&admin.id).await.unwrap();
        let got_owner = db.get_user_by_id(&owner.id).await.unwrap().unwrap();
        let got_admin = db.get_user_by_id(&admin.id).await.unwrap().unwrap();
        assert_eq!(got_owner.role, Role::Admin, "old owner should become admin");
        assert_eq!(got_admin.role, Role::Owner, "promoted user should be owner");
    }

    #[tokio::test]
    async fn session_round_trip_and_expiry_bump() {
        let db = in_memory_db().await;
        let user = make_user("sess@example.com", Role::Member);
        db.create_user(&user).await.unwrap();
        let s = Session {
            id: generate_session_token(),
            user_id: user.id.clone(),
            created_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::days(7),
            ip: None,
            user_agent: None,
            csrf_token: generate_csrf_token(),
        };
        db.create_session(&s).await.unwrap();
        // Kept by the token's hash (migration 044): the other methods take that.
        let stored = token_hash(&s.id);
        let got = db.get_session(&stored).await.unwrap().unwrap();
        assert_eq!(got.user_id, user.id);
        assert_eq!(got.csrf_token, s.csrf_token);
        let new_exp = chrono::Utc::now() + chrono::Duration::days(14);
        db.bump_session_expiry(&stored, new_exp).await.unwrap();
        let bumped = db.get_session(&stored).await.unwrap().unwrap();
        // allow 2 seconds of clock skew
        assert!(
            bumped.expires_at >= new_exp - chrono::Duration::seconds(2),
            "bumped expiry should be close to new_exp"
        );
    }

    #[tokio::test]
    async fn login_attempts_recent_filter() {
        let db = in_memory_db().await;
        db.record_login_attempt("joe@x.com", false).await.unwrap();
        db.record_login_attempt("joe@x.com", true).await.unwrap();
        let since = chrono::Utc::now() - chrono::Duration::minutes(15);
        let fails = db.recent_failed_login_attempts("joe@x.com", since).await.unwrap();
        assert_eq!(fails.len(), 1, "only failed attempts should be returned");
    }

    #[tokio::test]
    async fn invitation_create_get_use() {
        let db = in_memory_db().await;
        let inv = Invitation {
            id: Uuid::new_v4().to_string(),
            email: "invitee@example.com".to_string(),
            role: Role::Member,
            token: generate_session_token(),
            invited_by: None,
            org_id: crate::api::middleware::DEFAULT_ORG_ID.to_string(),
            expires_at: chrono::Utc::now() + chrono::Duration::days(7),
            used_at: None,
            created_at: chrono::Utc::now(),
        };
        db.create_invitation(&inv).await.unwrap();
        let got = db.get_invitation(&inv.token).await.unwrap().unwrap();
        assert_eq!(got.email, inv.email);
        assert!(got.used_at.is_none());
        db.mark_invitation_used(&inv.token).await.unwrap();
        let after = db.get_invitation(&inv.token).await.unwrap().unwrap();
        assert!(after.used_at.is_some(), "used_at should be set after mark_invitation_used");
    }

    /// An org's invitations not yet accepted, newest first; one is revoked
    /// by its id within its own org only, once.
    #[tokio::test]
    async fn an_orgs_unused_invitations_are_listed_and_revoked() {
        use crate::api::middleware::DEFAULT_ORG_ID;
        let db = in_memory_db().await;
        let now = chrono::Utc::now();
        db.create_organization(&crate::types::Organization {
            id: "org-other".into(),
            name: "Other".into(),
            slug: "other".into(),
            plan: "free".into(),
            status: "active".into(),
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();
        let invite = |org_id: &str, email: &str, age_hours: i64| Invitation {
            id: Uuid::new_v4().to_string(),
            email: email.to_string(),
            role: Role::Member,
            token: generate_session_token(),
            invited_by: None,
            org_id: org_id.to_string(),
            expires_at: now + chrono::Duration::days(7),
            used_at: None,
            created_at: now - chrono::Duration::hours(age_hours),
        };
        let older = invite(DEFAULT_ORG_ID, "older@example.com", 2);
        let newer = invite(DEFAULT_ORG_ID, "newer@example.com", 1);
        let used = invite(DEFAULT_ORG_ID, "used@example.com", 3);
        let theirs = invite("org-other", "theirs@example.com", 1);
        for inv in [&older, &newer, &used, &theirs] {
            db.create_invitation(inv).await.unwrap();
        }
        assert!(db.mark_invitation_used(&used.token).await.unwrap());

        let listed: Vec<String> = db.list_unused_invitations(DEFAULT_ORG_ID).await.unwrap().into_iter().map(|i| i.id).collect();
        assert_eq!(listed, [newer.id.clone(), older.id.clone()]);
        assert!(db.revoke_invitation(DEFAULT_ORG_ID, &theirs.id).await.unwrap().is_none(), "another org's");
        assert!(db.revoke_invitation(DEFAULT_ORG_ID, &used.id).await.unwrap().is_none(), "an accepted one");
        let gone = db.revoke_invitation(DEFAULT_ORG_ID, &older.id).await.unwrap().expect("revoked");
        assert_eq!(gone.email, "older@example.com");
        assert!(db.get_invitation(&older.token).await.unwrap().is_none(), "its token is gone");
        assert!(db.revoke_invitation(DEFAULT_ORG_ID, &older.id).await.unwrap().is_none(), "once");
        assert!(db.get_invitation(&theirs.token).await.unwrap().is_some(), "another org's stays");
    }

    fn reset_for(user_id: &str, token: &str) -> PasswordReset {
        PasswordReset {
            token: token.to_string(),
            user_id: user_id.to_string(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            used_at: None,
            created_at: chrono::Utc::now(),
            issued_by: None,
        }
    }

    fn invitation_with(token: &str) -> Invitation {
        Invitation {
            id: Uuid::new_v4().to_string(),
            email: format!("{}@example.com", Uuid::new_v4().simple()),
            role: Role::Member,
            token: token.to_string(),
            invited_by: None,
            org_id: crate::api::middleware::DEFAULT_ORG_ID.to_string(),
            expires_at: chrono::Utc::now() + chrono::Duration::days(7),
            used_at: None,
            created_at: chrono::Utc::now(),
        }
    }

    async fn password_hash_of(db: &SqliteDatabase, id: &str) -> Option<String> {
        db.get_user_by_id(id).await.unwrap().expect("the account").password_hash
    }

    /// Review A4: a reset token is stored only as its SHA-256, and found by
    /// the token itself; a new reset for the account voids its older unused
    /// one.
    #[tokio::test]
    async fn reset_tokens_are_stored_hashed_and_a_new_one_voids_the_older() {
        let db = in_memory_db().await;
        let user = make_user("reset@example.com", Role::Member);
        db.create_user(&user).await.unwrap();
        db.create_password_reset(&reset_for(&user.id, "first-token")).await.unwrap();
        let stored: Vec<String> = sqlx::query_scalar("SELECT token FROM password_resets").fetch_all(db.pool()).await.unwrap();
        assert_eq!(stored, [crate::auth::tokens::token_hash("first-token")], "only the hash is kept");
        assert!(db.get_password_reset("first-token").await.unwrap().is_some(), "found by the token");
        db.create_password_reset(&reset_for(&user.id, "second-token")).await.unwrap();
        assert!(db.get_password_reset("first-token").await.unwrap().is_none(), "the older one is void");
        assert!(db.get_password_reset("second-token").await.unwrap().is_some());
    }

    /// Review A4: confirming consumes the token before it writes the
    /// password, in one transaction. Round 3b, S1: deactivating deletes the
    /// account's unused links, and a link issued after that (by someone who
    /// may reactivate it) reactivates the account when confirmed.
    #[tokio::test]
    async fn a_reset_writes_a_password_only_with_an_unused_token_and_reactivates_the_account() {
        use crate::db::ResetConfirm;
        let db = in_memory_db().await;
        let user = make_user("confirm@example.com", Role::Member);
        db.create_user(&user).await.unwrap();

        db.create_password_reset(&reset_for(&user.id, "used-token")).await.unwrap();
        assert!(db.mark_password_reset_used("used-token").await.unwrap());
        assert_eq!(db.confirm_password_reset("used-token", "new-hash", crate::config::Mode::SingleTenant).await.unwrap(), ResetConfirm::Unusable);
        assert_eq!(password_hash_of(&db, &user.id).await, user.password_hash, "a used token writes no password");

        db.create_password_reset(&reset_for(&user.id, "before-token")).await.unwrap();
        db.update_user(&user.id, None, Some(false), None).await.unwrap();
        assert_eq!(db.confirm_password_reset("before-token", "new-hash", crate::config::Mode::SingleTenant).await.unwrap(), ResetConfirm::Unusable, "deactivating deleted it");
        assert_eq!(password_hash_of(&db, &user.id).await, user.password_hash);

        db.create_password_reset(&reset_for(&user.id, "after-token")).await.unwrap();
        assert_eq!(
            db.confirm_password_reset("after-token", "new-hash", crate::config::Mode::SingleTenant).await.unwrap(),
            ResetConfirm::Done { user_id: user.id.clone(), reactivated: true, revoked_invitations: vec![] }
        );
        let after = db.get_user_by_id(&user.id).await.unwrap().unwrap();
        assert_eq!((after.is_active, after.password_hash.as_deref()), (true, Some("new-hash")), "reactivated, with the new password");
        assert_eq!(db.confirm_password_reset("after-token", "other-hash", crate::config::Mode::SingleTenant).await.unwrap(), ResetConfirm::Unusable, "once");

        db.create_password_reset(&reset_for(&user.id, "active-token")).await.unwrap();
        assert_eq!(
            db.confirm_password_reset("active-token", "third-hash", crate::config::Mode::SingleTenant).await.unwrap(),
            ResetConfirm::Done { user_id: user.id.clone(), reactivated: false, revoked_invitations: vec![] }
        );
    }

    /// A session, an OAuth grant with an access token, and an approved code
    /// not yet redeemed, all `user_id`'s: (session's stored id, grant id,
    /// token hash, code hash).
    async fn sign_ins_for(db: &SqliteDatabase, user_id: &str) -> (String, String, String, String) {
        use crate::api::middleware::DEFAULT_ORG_ID;
        use crate::mcp::oauth::store::{AuthRequest, Decision, Grant, TokenKind, TokenRow};
        use crate::mcp::Surface;
        let now = chrono::Utc::now();
        let tag = Uuid::new_v4().simple().to_string();
        let session = format!("sess-{tag}");
        db.create_session(&Session { id: session.clone(), user_id: user_id.into(), created_at: now, expires_at: now + chrono::Duration::days(7), ip: None, user_agent: None, csrf_token: "csrf".into() })
            .await
            .unwrap();
        let oauth = db.mcp_oauth().expect("an OAuth store");
        let grant = format!("grant-{tag}");
        let scopes = vec!["admin:read".to_string()];
        oauth
            .insert_grant(&Grant { id: grant.clone(), client_id: format!("lgc-{tag}"), user_id: user_id.into(), org_id: DEFAULT_ORG_ID.into(), app_id: None, surface: Surface::Admin, scopes: scopes.clone(), created_at: now, last_used_at: None, revoked_at: None, sign_in_epoch: 0 })
            .await
            .unwrap();
        let token = format!("tok-{tag}");
        oauth
            .insert_token(&TokenRow { token_hash: token.clone(), grant_id: grant.clone(), kind: TokenKind::Access, created_at: now, expires_at: now + chrono::Duration::hours(1), rotated_at: None, revoked_at: None })
            .await
            .unwrap();
        let request = format!("req-{tag}");
        oauth
            .insert_request(&AuthRequest {
                id: request.clone(),
                client_id: format!("lgc-{tag}"),
                redirect_uri: "http://127.0.0.1/callback".into(),
                state: None,
                code_challenge: "challenge".into(),
                scopes: scopes.clone(),
                surface: Surface::Admin,
                created_at: now,
                expires_at: now + chrono::Duration::minutes(5),
                user_id: None,
                org_id: None,
                app_id: None,
                granted_scopes: None,
                code_hash: None,
                decided_at: None,
                consumed_at: None,
                sign_in_epoch: None,
            })
            .await
            .unwrap();
        let code = format!("code-{tag}");
        let approve = Decision::Approve { user_id: user_id.into(), org_id: DEFAULT_ORG_ID.into(), app_id: None, scopes, code_hash: code.clone(), code_expires_at: now + chrono::Duration::seconds(60), at: now, sign_in_epoch: 0 };
        assert!(oauth.decide_request(&request, approve).await.unwrap(), "approved");
        (token_hash(&session), grant, token, code)
    }

    /// Whether each of [`sign_ins_for`]'s sign-ins still works: the session,
    /// the grant, its token, and the code (still redeemable).
    async fn sign_ins_alive(db: &SqliteDatabase, (session, grant, token, code): &(String, String, String, String)) -> [bool; 4] {
        let oauth = db.mcp_oauth().expect("an OAuth store");
        let code_alive: Option<i64> = sqlx::query_scalar("SELECT 1 FROM mcp_oauth_requests WHERE code_hash = ? AND consumed_at IS NULL")
            .bind(code)
            .fetch_optional(db.pool())
            .await
            .unwrap();
        [
            db.get_session(session).await.unwrap().is_some(),
            oauth.get_grant(grant).await.unwrap().is_some_and(|g| g.revoked_at.is_none()),
            oauth.get_token(token).await.unwrap().is_some_and(|t| t.revoked_at.is_none()),
            code_alive.is_some(),
        ]
    }

    /// Round 3b, S1: deactivating an account ends its sessions, revokes its
    /// OAuth grants (and their tokens) and its codes not yet redeemed, and
    /// deletes its unused reset links, in the one transaction that
    /// deactivates it: a failure part way leaves all of it as it was.
    #[tokio::test]
    async fn deactivating_an_account_ends_its_sign_ins_and_links_at_once() {
        let db = in_memory_db().await;
        let user = make_user("leaving@example.com", Role::Member);
        db.create_user(&user).await.unwrap();
        let sign_ins = sign_ins_for(&db, &user.id).await;
        db.create_password_reset(&reset_for(&user.id, "pending-link")).await.unwrap();

        sqlx::query("ALTER TABLE mcp_oauth_requests RENAME TO mcp_oauth_requests_away").execute(db.pool()).await.unwrap();
        let failed = db.update_user(&user.id, None, Some(false), None).await;
        sqlx::query("ALTER TABLE mcp_oauth_requests_away RENAME TO mcp_oauth_requests").execute(db.pool()).await.unwrap();
        assert!(failed.is_err(), "the codes couldn't be deleted");
        assert!(db.get_user_by_id(&user.id).await.unwrap().unwrap().is_active, "so it is still active");
        assert_eq!(sign_ins_alive(&db, &sign_ins).await, [true; 4], "and everything it had stays");
        assert!(db.get_password_reset("pending-link").await.unwrap().is_some());

        db.update_user(&user.id, None, Some(false), None).await.unwrap().expect("deactivated");
        assert_eq!(sign_ins_alive(&db, &sign_ins).await, [false; 4], "session, grant, token, code");
        assert!(db.get_password_reset("pending-link").await.unwrap().is_none(), "the unused link is gone");
    }

    /// Round 3b, S2: confirming a reset ends every sign-in (sessions, OAuth
    /// grants and their tokens, codes not yet redeemed) in the transaction
    /// that writes the password and uses the token up: a failure part way
    /// leaves the password, the sign-ins and a usable link.
    #[tokio::test]
    async fn a_reset_ends_every_sign_in_at_once_or_not_at_all() {
        use crate::db::ResetConfirm;
        let db = in_memory_db().await;
        let user = make_user("stolen@example.com", Role::Member);
        db.create_user(&user).await.unwrap();
        let sign_ins = sign_ins_for(&db, &user.id).await;
        db.create_password_reset(&reset_for(&user.id, "rescue-link")).await.unwrap();

        sqlx::query("ALTER TABLE mcp_oauth_requests RENAME TO mcp_oauth_requests_away").execute(db.pool()).await.unwrap();
        let failed = db.confirm_password_reset("rescue-link", "new-hash", crate::config::Mode::SingleTenant).await;
        sqlx::query("ALTER TABLE mcp_oauth_requests_away RENAME TO mcp_oauth_requests").execute(db.pool()).await.unwrap();
        assert!(failed.is_err(), "the codes couldn't be deleted");
        assert_eq!(password_hash_of(&db, &user.id).await, user.password_hash, "no password written");
        assert_eq!(sign_ins_alive(&db, &sign_ins).await, [true; 4]);
        assert!(db.get_password_reset("rescue-link").await.unwrap().is_some_and(|r| r.used_at.is_none()), "the link stays usable");

        assert_eq!(db.confirm_password_reset("rescue-link", "new-hash", crate::config::Mode::SingleTenant).await.unwrap(), ResetConfirm::Done { user_id: user.id.clone(), reactivated: false, revoked_invitations: vec![] });
        assert_eq!(sign_ins_alive(&db, &sign_ins).await, [false; 4], "session, grant, token, code");
        assert_eq!(password_hash_of(&db, &user.id).await.as_deref(), Some("new-hash"));
    }

    /// Round 3b, S3: a link's permission was checked only when it was issued,
    /// so any change to the account's roles, ownership or memberships deletes
    /// its unused links (the triggers of migration 042), whichever code makes
    /// it: owner transfer and accepting an invitation were never edited for
    /// this. A used link stays, and an unrelated write (signing in) keeps them.
    #[tokio::test]
    async fn unused_reset_links_go_with_any_role_ownership_or_membership_change() {
        use crate::api::middleware::DEFAULT_ORG_ID;
        let db = in_memory_db().await;
        let user = make_user("moving@example.com", Role::Member);
        db.create_user(&user).await.unwrap();
        db.add_org_member(DEFAULT_ORG_ID, &user.id, Role::Member).await.unwrap();
        let other = crate::types::Organization {
            id: format!("org-{}", Uuid::new_v4().simple()),
            name: "Other".into(),
            slug: format!("other-{}", Uuid::new_v4().simple()),
            plan: "free".into(),
            status: "active".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        db.create_organization(&other).await.unwrap();
        let inv = Invitation { email: user.email.clone(), org_id: other.id.clone(), ..invitation_with("join-other") };
        db.create_invitation(&inv).await.unwrap();

        db.create_password_reset(&reset_for(&user.id, "used-link")).await.unwrap();
        assert!(db.mark_password_reset_used("used-link").await.unwrap());
        let changes = [
            "a role change in an org",
            "an account role change",
            "accepting an invitation to another org",
            "becoming that org's Owner",
            "becoming the deployment's Owner",
            "leaving an org",
            "joining an org",
            "being deactivated",
        ];
        for (n, what) in changes.into_iter().enumerate() {
            let token = format!("link-{n}");
            db.create_password_reset(&reset_for(&user.id, &token)).await.unwrap();
            db.touch_last_login(&user.id).await.unwrap();
            assert!(db.get_password_reset(&token).await.unwrap().is_some(), "{what}: signing in keeps the link");
            match n {
                0 => db.update_member_role(DEFAULT_ORG_ID, &user.id, Role::Viewer).await.unwrap(),
                1 => assert!(db.update_user(&user.id, Some(Role::Admin), None, None).await.unwrap().is_some()),
                2 => assert!(db.accept_invitation("join-other", None, &user.id, &other.id, Role::Member).await.unwrap()),
                3 => db.transfer_org_owner(&other.id, &user.id).await.unwrap(),
                4 => db.transfer_owner(&user.id).await.unwrap(),
                5 => db.remove_org_member(DEFAULT_ORG_ID, &user.id).await.unwrap(),
                6 => db.add_org_member(DEFAULT_ORG_ID, &user.id, Role::Member).await.unwrap(),
                _ => assert!(db.update_user(&user.id, None, Some(false), None).await.unwrap().is_some()),
            }
            assert!(db.get_password_reset(&token).await.unwrap().is_none(), "{what} deletes the unused link");
        }
        assert!(db.get_password_reset("used-link").await.unwrap().is_some(), "a used link stays");
    }

    /// Review A4: an invitation's token is stored only as its SHA-256, and
    /// found by the token itself; one stored before migration 041 (plain, no
    /// hash) is still found by its token until it expires.
    #[tokio::test]
    async fn invitation_tokens_are_stored_hashed_and_older_plain_ones_still_found() {
        use crate::api::middleware::DEFAULT_ORG_ID;
        let db = in_memory_db().await;
        let inv = invitation_with("plain-invite-token");
        db.create_invitation(&inv).await.unwrap();
        let stored: String = sqlx::query_scalar("SELECT token FROM invitations WHERE id = ?").bind(&inv.id).fetch_one(db.pool()).await.unwrap();
        assert_eq!(stored, crate::auth::tokens::token_hash("plain-invite-token"), "only the hash is kept");
        assert_eq!(db.get_invitation("plain-invite-token").await.unwrap().map(|i| i.id), Some(inv.id.clone()));
        assert!(db.get_invitation(&stored).await.unwrap().is_none(), "what is stored is no token");

        let now = chrono::Utc::now();
        sqlx::query("INSERT INTO invitations (id, email, role, token, org_id, expires_at, created_at) VALUES ('old-inv', 'old@example.com', 'member', 'legacy-token', ?, ?, ?)")
            .bind(DEFAULT_ORG_ID)
            .bind(now + chrono::Duration::days(7))
            .bind(now)
            .execute(db.pool())
            .await
            .unwrap();
        assert_eq!(db.get_invitation("legacy-token").await.unwrap().map(|i| i.id).as_deref(), Some("old-inv"), "a pre-041 row");
        assert!(db.mark_invitation_used("legacy-token").await.unwrap());
    }

    /// Review A6: accepting consumes the invitation first, in the same
    /// transaction as the account and the membership: one revoked meanwhile
    /// leaves neither.
    #[tokio::test]
    async fn accepting_an_invitation_revoked_meanwhile_writes_nothing() {
        use crate::api::middleware::DEFAULT_ORG_ID;
        let db = in_memory_db().await;
        let inv = invitation_with("revoked-token");
        db.create_invitation(&inv).await.unwrap();
        assert!(db.revoke_invitation(DEFAULT_ORG_ID, &inv.id).await.unwrap().is_some());
        let newcomer = make_user("newcomer@example.com", Role::Member);
        assert!(!db.accept_invitation("revoked-token", Some(&newcomer), &newcomer.id, DEFAULT_ORG_ID, Role::Member).await.unwrap());
        assert!(db.get_user_by_id(&newcomer.id).await.unwrap().is_none(), "no account");
        assert!(db.get_membership(DEFAULT_ORG_ID, &newcomer.id).await.unwrap().is_none(), "no membership");
    }

    /// Review A6: accepting and revoking at once end in one outcome: the
    /// invitation accepted, with its account and membership, and the revoke
    /// finding nothing to revoke; or revoked, with neither.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn accepting_and_revoking_at_once_end_in_one_outcome() {
        use crate::api::middleware::DEFAULT_ORG_ID;
        let db = std::sync::Arc::new(in_memory_db().await);
        for round in 0..20 {
            let token = format!("race-token-{round}");
            let inv = invitation_with(&token);
            db.create_invitation(&inv).await.unwrap();
            let newcomer = make_user(&format!("racer{round}@example.com"), Role::Member);
            let start = std::sync::Arc::new(tokio::sync::Barrier::new(2));
            let accept = {
                let (db, start, newcomer) = (db.clone(), start.clone(), newcomer.clone());
                tokio::spawn(async move {
                    start.wait().await;
                    db.accept_invitation(&token, Some(&newcomer), &newcomer.id, DEFAULT_ORG_ID, Role::Member).await
                })
            };
            let revoke = {
                let (db, start, id) = (db.clone(), start.clone(), inv.id.clone());
                tokio::spawn(async move {
                    start.wait().await;
                    db.revoke_invitation(DEFAULT_ORG_ID, &id).await
                })
            };
            let accepted = accept.await.unwrap().expect("accept");
            let revoked = revoke.await.unwrap().expect("revoke").is_some();
            let member = db.get_membership(DEFAULT_ORG_ID, &newcomer.id).await.unwrap().is_some();
            let account = db.get_user_by_id(&newcomer.id).await.unwrap().is_some();
            assert!(accepted != revoked, "round {round}: exactly one wins (accepted {accepted}, revoked {revoked})");
            assert_eq!((member, account), (accepted, accepted), "round {round}: the account and membership go with acceptance");
        }
    }

    fn session_for(user_id: &str, id: &str) -> Session {
        let now = chrono::Utc::now();
        Session { id: id.to_string(), user_id: user_id.to_string(), created_at: now, expires_at: now + chrono::Duration::days(7), ip: None, user_agent: None, csrf_token: "csrf".into() }
    }

    /// Review round 4, finding 2: a sign-in begun before a reset, a
    /// deactivation or a change of one's own password commits (a login reads
    /// the account's sign-in epoch, checks the password, then inserts its
    /// session) is refused once it lands, and a session begun after works.
    #[tokio::test]
    async fn a_sign_in_begun_before_a_reset_deactivation_or_password_change_is_refused() {
        use crate::config::Mode;
        use crate::db::ResetConfirm;
        let db = in_memory_db().await;
        let user = make_user("straddle@example.com", Role::Member);
        db.create_user(&user).await.unwrap();

        let begun = db.sign_in_epoch(&user.id).await.unwrap().expect("the account");
        db.create_password_reset(&reset_for(&user.id, "rescue")).await.unwrap();
        assert!(matches!(db.confirm_password_reset("rescue", "new-hash", Mode::SingleTenant).await.unwrap(), ResetConfirm::Done { .. }));
        db.create_session_at_epoch(&session_for(&user.id, "late-login"), begun).await.unwrap();
        assert!(db.get_session_with_user("late-login").await.unwrap().is_none(), "a login begun before the reset");
        let now = db.sign_in_epoch(&user.id).await.unwrap().unwrap();
        db.create_session_at_epoch(&session_for(&user.id, "fresh-login"), now).await.unwrap();
        assert!(db.get_session_with_user("fresh-login").await.unwrap().is_some(), "a login begun after it works");

        let begun = db.sign_in_epoch(&user.id).await.unwrap().unwrap();
        db.update_user(&user.id, None, Some(false), None).await.unwrap();
        db.update_user(&user.id, None, Some(true), None).await.unwrap();
        db.create_session_at_epoch(&session_for(&user.id, "across-deactivation"), begun).await.unwrap();
        assert!(db.get_session_with_user("across-deactivation").await.unwrap().is_none(), "a login begun before a deactivation");

        let begun = db.sign_in_epoch(&user.id).await.unwrap().unwrap();
        db.create_session(&session_for(&user.id, "kept")).await.unwrap();
        assert_eq!(db.change_own_password(&user.id, "newer-hash", "new-hash", &token_hash("kept")).await.unwrap(), crate::db::PasswordChange::Done);
        db.create_session_at_epoch(&session_for(&user.id, "across-password-change"), begun).await.unwrap();
        assert!(db.get_session_with_user("across-password-change").await.unwrap().is_none(), "a login begun before a password change");
        assert!(db.get_session_with_user("kept").await.unwrap().is_some(), "the session that changed it stays valid");
    }

    /// Review round 4, finding 3: writing is_active false deletes the unused
    /// links every time, also on an account already inactive, so a link
    /// issued to reactivate it can be killed by deactivating it again.
    #[tokio::test]
    async fn deactivating_an_inactive_account_again_deletes_its_links() {
        let db = in_memory_db().await;
        let user = make_user("twice@example.com", Role::Member);
        db.create_user(&user).await.unwrap();
        db.update_user(&user.id, None, Some(false), None).await.unwrap();
        db.create_password_reset(&reset_for(&user.id, "to-reactivate")).await.unwrap();
        db.update_user(&user.id, None, Some(false), None).await.unwrap();
        assert!(db.get_password_reset("to-reactivate").await.unwrap().is_none(), "deactivating again deletes it");
    }

    /// An account in `org` with `role` (and the default org's membership too,
    /// as single-tenant accounts have).
    async fn member_of(db: &SqliteDatabase, email: &str, role: Role) -> User {
        let user = make_user(email, role);
        db.create_user(&user).await.unwrap();
        db.add_org_member(crate::api::middleware::DEFAULT_ORG_ID, &user.id, role).await.unwrap();
        user
    }

    /// Review round 4, findings 4 and 5: confirming re-checks that the link's
    /// issuer may still issue it, inside the transaction: still active, still
    /// outranking the account everywhere, and for a deactivated account still
    /// allowed to reactivate it. A promotion that committed between the issue
    /// checks and the link's insert (so 042's trigger found no link), or an
    /// issuer demoted or deactivated since, makes the link refuse. The master
    /// key's links (no issuer) always confirm.
    #[tokio::test]
    async fn a_link_whose_issuer_may_no_longer_issue_it_is_refused() {
        use crate::api::middleware::DEFAULT_ORG_ID;
        use crate::config::Mode;
        use crate::db::ResetConfirm;
        let db = in_memory_db().await;
        let admin = member_of(&db, "admin@example.com", Role::Admin).await;
        let link = |target: &str, token: &str, issuer: Option<&str>| PasswordReset { issued_by: issuer.map(str::to_string), ..reset_for(target, token) };

        let promoted = member_of(&db, "promoted@example.com", Role::Member).await;
        db.update_member_role(DEFAULT_ORG_ID, &promoted.id, Role::Admin).await.unwrap();
        db.create_password_reset(&link(&promoted.id, "late-insert", Some(&admin.id))).await.unwrap();
        assert_eq!(db.confirm_password_reset("late-insert", "h", Mode::SingleTenant).await.unwrap(), ResetConfirm::IssuerRevoked, "the target now ranks with the issuer");
        assert!(db.get_password_reset("late-insert").await.unwrap().is_some_and(|r| r.used_at.is_none()), "nothing changed");

        let member = member_of(&db, "member@example.com", Role::Member).await;
        let demoted = member_of(&db, "demoted@example.com", Role::Admin).await;
        db.update_member_role(DEFAULT_ORG_ID, &demoted.id, Role::Member).await.unwrap();
        db.create_password_reset(&link(&member.id, "by-demoted", Some(&demoted.id))).await.unwrap();
        assert_eq!(db.confirm_password_reset("by-demoted", "h", Mode::SingleTenant).await.unwrap(), ResetConfirm::IssuerRevoked, "a demoted issuer");

        let gone = member_of(&db, "gone@example.com", Role::Admin).await;
        db.update_user(&gone.id, None, Some(false), None).await.unwrap();
        db.create_password_reset(&link(&member.id, "by-deactivated", Some(&gone.id))).await.unwrap();
        assert_eq!(db.confirm_password_reset("by-deactivated", "h", Mode::SingleTenant).await.unwrap(), ResetConfirm::IssuerRevoked, "a deactivated issuer");

        let inactive = member_of(&db, "inactive@example.com", Role::Member).await;
        db.update_user(&inactive.id, None, Some(false), None).await.unwrap();
        db.create_password_reset(&link(&inactive.id, "hosted-reactivation", Some(&admin.id))).await.unwrap();
        assert_eq!(db.confirm_password_reset("hosted-reactivation", "h", Mode::Hosted).await.unwrap(), ResetConfirm::IssuerRevoked, "hosted: a person can't reactivate");
        db.create_password_reset(&link(&inactive.id, "reactivation", Some(&admin.id))).await.unwrap();
        assert_eq!(
            db.confirm_password_reset("reactivation", "h", Mode::SingleTenant).await.unwrap(),
            ResetConfirm::Done { user_id: inactive.id.clone(), reactivated: true, revoked_invitations: vec![] },
            "single-tenant: an Admin may reactivate a Member"
        );

        db.create_password_reset(&link(&member.id, "by-admin", Some(&admin.id))).await.unwrap();
        assert!(matches!(db.confirm_password_reset("by-admin", "h", Mode::SingleTenant).await.unwrap(), ResetConfirm::Done { .. }), "an Admin, for a Member");
        db.create_password_reset(&link(&promoted.id, "by-master-key", None)).await.unwrap();
        assert!(matches!(db.confirm_password_reset("by-master-key", "h", Mode::SingleTenant).await.unwrap(), ResetConfirm::Done { .. }), "the master key, for anyone");
    }

    /// Review round 4, finding 5: an issuer's unused links go when the issuer
    /// is demoted, removed or deactivated (043's triggers), as the
    /// invitations they sent do.
    #[tokio::test]
    async fn an_issuers_unused_links_go_when_the_issuer_is_demoted_removed_or_deactivated() {
        use crate::api::middleware::DEFAULT_ORG_ID;
        let db = in_memory_db().await;
        let member = member_of(&db, "member@example.com", Role::Member).await;
        let link = |token: &str, issuer: &str| PasswordReset { issued_by: Some(issuer.to_string()), ..reset_for(&member.id, token) };
        for (n, what) in ["demoted", "removed", "deactivated"].into_iter().enumerate() {
            let issuer = member_of(&db, &format!("issuer{n}@example.com"), Role::Admin).await;
            let token = format!("link-{what}");
            db.create_password_reset(&link(&token, &issuer.id)).await.unwrap();
            match n {
                0 => db.update_member_role(DEFAULT_ORG_ID, &issuer.id, Role::Member).await.unwrap(),
                1 => db.remove_org_member(DEFAULT_ORG_ID, &issuer.id).await.unwrap(),
                _ => assert!(db.update_user(&issuer.id, None, Some(false), None).await.unwrap().is_some()),
            }
            assert!(db.get_password_reset(&token).await.unwrap().is_none(), "an issuer {what}: its link goes");
        }
    }

    /// Review round 5, finding 1: a password change whose check passed
    /// before a reset committed writes nothing: the write holds only while
    /// the account still has the hash the current password was checked
    /// against, is active, and is at the sending session's sign-in epoch.
    /// A thief looping PATCH /v1/account can't overwrite the reset's password.
    #[tokio::test]
    async fn a_password_change_checked_before_a_reset_does_not_overwrite_it() {
        use crate::config::Mode;
        use crate::db::{PasswordChange, ResetConfirm};
        let db = in_memory_db().await;
        let user = make_user("racing@example.com", Role::Member);
        db.create_user(&user).await.unwrap();
        db.create_session(&session_for(&user.id, "thief-session")).await.unwrap();
        let verified = user.password_hash.clone().expect("a password");
        db.create_password_reset(&reset_for(&user.id, "rescue")).await.unwrap();
        assert!(matches!(db.confirm_password_reset("rescue", "reset-hash", Mode::SingleTenant).await.unwrap(), ResetConfirm::Done { .. }));
        assert_eq!(db.change_own_password(&user.id, "thief-hash", &verified, &token_hash("thief-session")).await.unwrap(), PasswordChange::SignInEnded);
        assert_eq!(password_hash_of(&db, &user.id).await.as_deref(), Some("reset-hash"), "the reset's password stays");

        db.create_session(&session_for(&user.id, "owner-session")).await.unwrap();
        let owner_session = token_hash("owner-session");
        assert_eq!(db.change_own_password(&user.id, "stale-check", &verified, &owner_session).await.unwrap(), PasswordChange::SignInEnded, "checked against a hash that moved");
        assert_eq!(db.change_own_password(&user.id, "owners-new", "reset-hash", &owner_session).await.unwrap(), PasswordChange::Done, "a change checked now");
        assert_eq!(password_hash_of(&db, &user.id).await.as_deref(), Some("owners-new"));
        db.update_user(&user.id, None, Some(false), None).await.unwrap();
        db.update_user(&user.id, None, Some(true), None).await.unwrap();
        db.create_session(&session_for(&user.id, "after-deactivation")).await.unwrap();
        db.update_user(&user.id, None, Some(false), None).await.unwrap();
        assert_eq!(db.change_own_password(&user.id, "while-inactive", "owners-new", &token_hash("after-deactivation")).await.unwrap(), PasswordChange::SignInEnded, "an inactive account");
    }

    /// Review round 5, finding 2 (the reactivation half): a session stamped
    /// while the account was deactivated doesn't come back with its
    /// reactivation, which moves the sign-in epoch on too.
    #[tokio::test]
    async fn reactivating_an_account_moves_its_sign_in_epoch_on() {
        let db = in_memory_db().await;
        let user = make_user("back@example.com", Role::Member);
        db.create_user(&user).await.unwrap();
        db.update_user(&user.id, None, Some(false), None).await.unwrap();
        let while_inactive = db.sign_in_epoch(&user.id).await.unwrap().unwrap();
        db.create_session_at_epoch(&session_for(&user.id, "stamped-while-inactive"), while_inactive).await.unwrap();
        db.update_user(&user.id, None, Some(true), None).await.unwrap();
        assert!(db.get_session_with_user("stamped-while-inactive").await.unwrap().is_none(), "refused after the reactivation");
        let active = db.sign_in_epoch(&user.id).await.unwrap().unwrap();
        db.update_user(&user.id, None, Some(true), None).await.unwrap();
        assert_eq!(db.sign_in_epoch(&user.id).await.unwrap(), Some(active), "an active account written active again keeps its epoch");
    }

    /// Review round 5, findings 3 and 4: an account's reset, deactivation or
    /// own password change also deletes the unused links it issued (from a
    /// stolen session, say); a reset and a deactivation (not a routine
    /// password change) also revoke the pending invitations it sent, and
    /// the confirm hands them back for the audit log.
    #[tokio::test]
    async fn ending_an_accounts_sign_ins_deletes_the_links_it_issued_and_a_reset_revokes_its_invitations() {
        use crate::config::Mode;
        use crate::db::{PasswordChange, ResetConfirm};
        let db = in_memory_db().await;
        let admin = member_of(&db, "stolen-admin@example.com", Role::Admin).await;
        let member = member_of(&db, "target@example.com", Role::Member).await;
        let issued = |token: &str| PasswordReset { issued_by: Some(admin.id.clone()), ..reset_for(&member.id, token) };
        let invited = Invitation { invited_by: Some(admin.id.clone()), ..invitation_with("stand-in") };

        db.create_password_reset(&issued("before-reset")).await.unwrap();
        db.create_invitation(&invited).await.unwrap();
        db.create_password_reset(&reset_for(&admin.id, "admins-rescue")).await.unwrap();
        let ResetConfirm::Done { revoked_invitations, .. } = db.confirm_password_reset("admins-rescue", "new-admin-hash", Mode::SingleTenant).await.unwrap() else {
            panic!("the reset went through");
        };
        assert!(db.get_password_reset("before-reset").await.unwrap().is_none(), "a reset deletes the links the account issued");
        assert_eq!(revoked_invitations.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), [invited.id.as_str()], "and revokes its invitations");
        assert!(db.get_invitation("stand-in").await.unwrap().is_none());

        db.create_password_reset(&issued("before-password-change")).await.unwrap();
        let second = Invitation { invited_by: Some(admin.id.clone()), ..invitation_with("kept-invite") };
        db.create_invitation(&second).await.unwrap();
        db.create_session(&session_for(&admin.id, "admins-session")).await.unwrap();
        assert_eq!(db.change_own_password(&admin.id, "newer-admin-hash", "new-admin-hash", &token_hash("admins-session")).await.unwrap(), PasswordChange::Done);
        assert!(db.get_password_reset("before-password-change").await.unwrap().is_none(), "a password change deletes them too");
        assert!(db.get_invitation("kept-invite").await.unwrap().is_some(), "a routine password change keeps the invitations");
    }

    /// Review round 5, finding 5: a stale claim (made under an epoch older
    /// than the person's) revokes nothing and is refused: it can't take down
    /// the grant the person made since.
    #[tokio::test]
    async fn a_stale_grant_claim_revokes_nothing_and_is_refused() {
        use crate::api::middleware::DEFAULT_ORG_ID;
        use crate::mcp::oauth::store::Grant;
        use crate::mcp::Surface;
        let db = in_memory_db().await;
        let user = make_user("claims@example.com", Role::Member);
        db.create_user(&user).await.unwrap();
        sqlx::query("UPDATE users SET sign_in_epoch = 1 WHERE id = ?").bind(&user.id).execute(db.pool()).await.unwrap();
        let oauth = db.mcp_oauth().expect("an OAuth store");
        let now = chrono::Utc::now();
        let grant = |id: &str, epoch: i64| Grant {
            id: id.to_string(),
            client_id: "lgc-claims".into(),
            user_id: user.id.clone(),
            org_id: DEFAULT_ORG_ID.into(),
            app_id: None,
            surface: Surface::Ai,
            scopes: vec!["ai:read".into()],
            created_at: now,
            last_used_at: None,
            revoked_at: None,
            sign_in_epoch: epoch,
        };
        assert_eq!(oauth.claim_grant(&grant("newer", 1)).await.unwrap().map(|g| g.id).as_deref(), Some("newer"));
        assert_eq!(oauth.claim_grant(&grant("stale", 0)).await.unwrap(), None, "a claim older than the person's epoch");
        assert!(oauth.get_grant("newer").await.unwrap().is_some_and(|g| g.revoked_at.is_none()), "the newer grant stays");
        assert!(oauth.get_grant("stale").await.unwrap().is_none(), "no stale grant made");
    }

    /// Review round 5, finding 6: the account's session list shows only the
    /// sessions of its current sign-in epoch; one refused by the session
    /// check isn't listed as if still signed in.
    #[tokio::test]
    async fn the_session_list_leaves_out_sessions_of_an_old_epoch() {
        let db = in_memory_db().await;
        let user = make_user("listed@example.com", Role::Member);
        db.create_user(&user).await.unwrap();
        db.create_session_at_epoch(&session_for(&user.id, "old-epoch"), -1).await.unwrap();
        db.create_session(&session_for(&user.id, "current")).await.unwrap();
        let listed: Vec<String> = db.list_user_sessions(&user.id).await.unwrap().into_iter().map(|s| s.id).collect();
        assert_eq!(listed, [token_hash("current")], "by its stored id");
    }

    async fn link_stored(db: &SqliteDatabase, token: &str) -> bool {
        db.get_password_reset(token).await.unwrap().is_some()
    }

    /// Review round 6, finding 2: a link or an invitation is stored only
    /// while the sign-in sending it (a session, or an app's OAuth grant) is
    /// live, checked in the statement that stores it. Once a reset, an epoch
    /// move or a deactivation ended it, nothing is stored, and a refused
    /// link leaves the account's earlier link alone. The master key's are
    /// unconditional.
    #[tokio::test]
    async fn a_link_or_invitation_from_an_ended_sign_in_is_not_stored() {
        use crate::api::middleware::DEFAULT_ORG_ID;
        use crate::config::Mode;
        use crate::db::SignIn;
        use crate::mcp::oauth::store::Grant;
        use crate::mcp::Surface;
        let db = in_memory_db().await;
        let admin = member_of(&db, "sender@example.com", Role::Admin).await;
        let member = member_of(&db, "account@example.com", Role::Member).await;
        let link = |token: &str| PasswordReset { issued_by: Some(admin.id.clone()), ..reset_for(&member.id, token) };
        let invitation = |token: &str| Invitation { invited_by: Some(admin.id.clone()), ..invitation_with(token) };
        let oauth = db.mcp_oauth().expect("an OAuth store");
        let grant_at = |id: &str, epoch: i64| Grant {
            id: id.to_string(),
            client_id: format!("lgc-{id}"),
            user_id: admin.id.clone(),
            org_id: DEFAULT_ORG_ID.into(),
            app_id: None,
            surface: Surface::Ai,
            scopes: vec!["ai:read".into()],
            created_at: chrono::Utc::now(),
            last_used_at: None,
            revoked_at: None,
            sign_in_epoch: epoch,
        };

        // A session's sign-in goes by its stored id (migration 044).
        let (live, straddling, inactive) = (token_hash("live-session"), token_hash("straddling-session"), token_hash("inactive-session"));
        db.create_session(&session_for(&admin.id, "live-session")).await.unwrap();
        assert!(oauth.claim_grant(&grant_at("live-grant", 0)).await.unwrap().is_some());
        for (from, token) in [(SignIn::Session(&live), "while-signed-in"), (SignIn::Grant("live-grant"), "while-the-app-is-signed-in")] {
            assert!(db.create_invitation_from(&invitation(token), from).await.unwrap(), "{from:?}: an invitation from a live sign-in");
            assert!(db.create_password_reset_from(&link(token), from).await.unwrap(), "{from:?}: a link from a live sign-in");
            assert!(db.get_invitation(token).await.unwrap().is_some() && link_stored(&db, token).await, "{from:?}: both stored");
        }

        // A reset ends both sign-ins; what they send after it isn't stored.
        db.create_password_reset(&reset_for(&member.id, "the-accounts-own")).await.unwrap();
        db.create_password_reset(&reset_for(&admin.id, "rescue")).await.unwrap();
        assert!(matches!(db.confirm_password_reset("rescue", "reset-hash", Mode::SingleTenant).await.unwrap(), crate::db::ResetConfirm::Done { .. }));
        for (from, token) in [(SignIn::Session(&live), "after-the-reset"), (SignIn::Grant("live-grant"), "after-the-reset-from-the-app")] {
            assert!(!db.create_invitation_from(&invitation(token), from).await.unwrap(), "{from:?}: an invitation refused");
            assert!(!db.create_password_reset_from(&link(token), from).await.unwrap(), "{from:?}: a link refused");
            assert!(db.get_invitation(token).await.unwrap().is_none() && !link_stored(&db, token).await, "{from:?}: nothing stored");
        }
        assert!(link_stored(&db, "the-accounts-own").await, "a refused link voids no earlier link");

        // A session and a grant left at an epoch the account has moved past
        // (an end of sign-ins they straddled) are refused too.
        db.create_session(&session_for(&admin.id, "straddling-session")).await.unwrap();
        let current = db.sign_in_epoch(&admin.id).await.unwrap().unwrap();
        assert!(oauth.claim_grant(&grant_at("straddling-grant", current)).await.unwrap().is_some());
        sqlx::query("UPDATE users SET sign_in_epoch = sign_in_epoch + 1 WHERE id = ?").bind(&admin.id).execute(db.pool()).await.unwrap();
        for from in [SignIn::Session(&straddling), SignIn::Grant("straddling-grant")] {
            assert!(!db.create_invitation_from(&invitation("from-an-old-epoch"), from).await.unwrap(), "{from:?}: an old epoch");
        }

        // A deactivation: even a session at the current epoch of an inactive
        // account sends nothing, and neither does one that is gone.
        db.update_user(&admin.id, None, Some(false), None).await.unwrap();
        let while_inactive = db.sign_in_epoch(&admin.id).await.unwrap().unwrap();
        db.create_session_at_epoch(&session_for(&admin.id, "inactive-session"), while_inactive).await.unwrap();
        assert!(!db.create_invitation_from(&invitation("while-deactivated"), SignIn::Session(&inactive)).await.unwrap(), "an inactive account's session");
        assert!(!db.create_password_reset_from(&link("while-deactivated"), SignIn::Session(&inactive)).await.unwrap(), "an inactive account's session");
        assert!(!db.create_invitation_from(&invitation("no-such-session"), SignIn::Session("no-such-session")).await.unwrap(), "a session that is gone");
        assert!(db.get_invitation("while-deactivated").await.unwrap().is_none() && !link_stored(&db, "while-deactivated").await);

        // The master key's are stored whatever the sender's standing.
        assert!(db.create_invitation_from(&invitation("from-the-master-key"), SignIn::MasterKey).await.unwrap());
        assert!(db.create_password_reset_from(&reset_for(&member.id, "from-the-master-key"), SignIn::MasterKey).await.unwrap());
        assert!(db.get_invitation("from-the-master-key").await.unwrap().is_some() && link_stored(&db, "from-the-master-key").await);
        assert!(!link_stored(&db, "the-accounts-own").await, "a new link voids the account's older unused one");
    }

    /// Review round 6, finding 2: a deactivation revokes the pending
    /// invitations the account sent, in every org, in its own transaction,
    /// and hands them back for the audit log.
    #[tokio::test]
    async fn a_deactivation_revokes_the_accounts_invitations_in_its_transaction() {
        let db = in_memory_db().await;
        let admin = member_of(&db, "leaving@example.com", Role::Admin).await;
        let other = crate::types::Organization {
            id: format!("org-{}", Uuid::new_v4().simple()),
            name: "Other".into(),
            slug: format!("other-{}", Uuid::new_v4().simple()),
            plan: "free".into(),
            status: "active".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        db.create_organization(&other).await.unwrap();
        let sent = Invitation { invited_by: Some(admin.id.clone()), ..invitation_with("sent-here") };
        let elsewhere = Invitation { invited_by: Some(admin.id.clone()), org_id: other.id.clone(), ..invitation_with("sent-elsewhere") };
        let others = invitation_with("someone-elses");
        for inv in [&sent, &elsewhere, &others] {
            db.create_invitation(inv).await.unwrap();
        }
        let (user, revoked) = db.update_user_with_revoked(&admin.id, None, Some(false), None, None).await.unwrap().expect("the account");
        assert!(!user.is_active);
        let mut ids: Vec<&str> = revoked.iter().map(|i| i.id.as_str()).collect();
        ids.sort();
        let mut want = [sent.id.as_str(), elsewhere.id.as_str()];
        want.sort();
        assert_eq!(ids, want, "both of the account's, in every org");
        assert!(db.get_invitation("sent-here").await.unwrap().is_none() && db.get_invitation("sent-elsewhere").await.unwrap().is_none());
        assert!(db.get_invitation("someone-elses").await.unwrap().is_some(), "another sender's stays");

        let again = Invitation { invited_by: Some(admin.id.clone()), ..invitation_with("sent-while-inactive") };
        db.create_invitation(&again).await.unwrap();
        let (_, revoked) = db.update_user_with_revoked(&admin.id, Some(Role::Member), None, None, None).await.unwrap().expect("the account");
        assert!(revoked.is_empty(), "only a deactivation revokes them");
        db.update_user(&admin.id, None, Some(false), None).await.unwrap();
        assert!(db.get_invitation("sent-while-inactive").await.unwrap().is_none(), "update_user's deactivation revokes them too");
    }

    /// Review of round 8: a role change with its membership moves both roles,
    /// and never an Owner's. An account made the Owner after the caller checked
    /// stays the Owner: the change fails and writes nothing, so a retry is
    /// refused as the Owner's.
    #[tokio::test]
    async fn a_role_change_with_its_membership_never_demotes_an_owner() {
        let db = in_memory_db().await;
        let org = crate::api::middleware::DEFAULT_ORG_ID;
        let admin = member_of(&db, "admin@example.com", Role::Admin).await;
        let (user, _) = db.update_user_with_revoked(&admin.id, Some(Role::Member), None, None, Some(org)).await.unwrap().expect("the account");
        assert_eq!((user.role, db.get_membership(org, &admin.id).await.unwrap()), (Role::Member, Some(Role::Member)), "both moved");
        let owner = member_of(&db, "owner@example.com", Role::Owner).await;
        assert!(db.update_user_with_revoked(&owner.id, Some(Role::Member), None, None, Some(org)).await.is_err(), "an Owner's role isn't changed");
        let after = db.get_user_by_id(&owner.id).await.unwrap().unwrap();
        assert_eq!((after.role, db.get_membership(org, &owner.id).await.unwrap()), (Role::Owner, Some(Role::Owner)), "nothing written");
    }

    #[tokio::test]
    async fn link_oauth_and_get_user_by_oauth() {
        let db = in_memory_db().await;
        let user = make_user("oauth@example.com", Role::Member);
        db.create_user(&user).await.unwrap();
        let gh_id = "github-id-12345";
        db.link_oauth(&user.id, "github", gh_id).await.unwrap();
        let found = db.get_user_by_oauth("github", gh_id).await.unwrap();
        assert!(found.is_some(), "should find user by oauth id");
        assert_eq!(found.unwrap().id, user.id);
    }

    #[tokio::test]
    async fn password_reset_round_trip() {
        let db = in_memory_db().await;
        let user = make_user("reset@example.com", Role::Member);
        db.create_user(&user).await.unwrap();
        let reset = PasswordReset {
            token: generate_session_token(),
            user_id: user.id.clone(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            used_at: None,
            created_at: chrono::Utc::now(),
            issued_by: None,
        };
        db.create_password_reset(&reset).await.unwrap();
        let got = db.get_password_reset(&reset.token).await.unwrap().unwrap();
        assert_eq!(got.user_id, user.id);
        assert!(got.used_at.is_none());
        db.mark_password_reset_used(&reset.token).await.unwrap();
        let after = db.get_password_reset(&reset.token).await.unwrap().unwrap();
        assert!(after.used_at.is_some(), "used_at should be set after mark");
    }

    #[tokio::test]
    async fn delete_user_sessions_respects_except_id() {
        let db = in_memory_db().await;
        let user = make_user("multi-sess@example.com", Role::Member);
        db.create_user(&user).await.unwrap();
        let sess1 = Session {
            id: generate_session_token(),
            user_id: user.id.clone(),
            created_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::days(7),
            ip: None, user_agent: None,
            csrf_token: generate_csrf_token(),
        };
        let sess2 = Session {
            id: generate_session_token(),
            user_id: user.id.clone(),
            created_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::days(7),
            ip: None, user_agent: None,
            csrf_token: generate_csrf_token(),
        };
        db.create_session(&sess1).await.unwrap();
        db.create_session(&sess2).await.unwrap();
        // delete all except sess1 (each by its stored id: its token's hash)
        let (stored1, stored2) = (token_hash(&sess1.id), token_hash(&sess2.id));
        assert!(db.get_session(&stored2).await.unwrap().is_some(), "sess2 is stored");
        let deleted = db.delete_user_sessions(&user.id, Some(&stored1)).await.unwrap();
        assert_eq!(deleted, 1, "should have deleted 1 session");
        assert!(db.get_session(&stored1).await.unwrap().is_some(), "sess1 should survive");
        assert!(db.get_session(&stored2).await.unwrap().is_none(), "sess2 should be deleted");
    }

    /// Verifies migration 0008 applies on a fresh DB: the multitenant tables are
    /// created and the backfill inserts the default org/app rows. This connects a
    /// fresh pool and runs ALL migrations (including 0008) directly, then raw-queries
    /// the new tables — a SQL error in 0008 would fail at migrate-time.
    #[tokio::test]
    async fn migration_0008_creates_tenant_tables() {
        use sqlx::sqlite::SqlitePoolOptions;

        // shared-cache in-memory DB so every pooled connection sees the same schema.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite:file:mig0008?mode=memory&cache=shared")
            .await
            .expect("connect fresh sqlite");
        sqlx::migrate!("./migrations/sqlite")
            .run(&pool)
            .await
            .expect("migration 0008 must apply on a fresh DB");

        // All three new tables must exist and be queryable.
        for table in ["organizations", "applications", "organization_members"] {
            let sql = format!("SELECT count(*) FROM {table}");
            sqlx::query_scalar::<_, i64>(&sql)
                .fetch_one(&pool)
                .await
                .unwrap_or_else(|e| panic!("table {table} must exist: {e}"));
        }

        // The default org row from the backfill must be present.
        let org_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM organizations WHERE id = '00000000-0000-0000-0000-000000000001'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(org_count, 1, "default org row must exist after backfill");

        // The default app row from the backfill must be present.
        let app_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM applications WHERE id = '00000000-0000-0000-0000-000000000002'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(app_count, 1, "default app row must exist after backfill");

        // Tenant columns must exist on the altered tables.
        for table in ["api_keys", "generations", "request_logs", "audit_log", "invitations",
                      "request_artifacts", "webhook_deliveries"] {
            let sql = format!("SELECT org_id FROM {table} LIMIT 1");
            sqlx::query(&sql)
                .fetch_optional(&pool)
                .await
                .unwrap_or_else(|e| panic!("{table}.org_id column must exist: {e}"));
        }
        // public_id added only on api_keys.
        sqlx::query("SELECT public_id FROM api_keys LIMIT 1")
            .fetch_optional(&pool)
            .await
            .expect("api_keys.public_id column must exist");
    }

    /// Before 2026-11-06 a key made by a key got no owner, so it outlived the
    /// person behind it. Migration 025 gives each such key its maker's owner,
    /// read back through the audit log's `key.create` rows, down any chain of
    /// keys. Keys made with the master key (no actor) stay the org's, an owner
    /// already set is kept, and a cycle in the log ends.
    #[tokio::test]
    async fn migration_0025_gives_keys_made_by_keys_their_makers_owner() {
        use sqlx::sqlite::SqlitePoolOptions;

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite:file:mig0025?mode=memory&cache=shared")
            .await
            .expect("connect fresh sqlite");
        let mut before = sqlx::migrate!("./migrations/sqlite");
        before.migrations =
            std::borrow::Cow::Owned(before.migrations.iter().filter(|m| m.version < 20240101000025).cloned().collect());
        before.run(&pool).await.expect("migrations before 025");

        for user in ["u-maker", "u-other"] {
            sqlx::query("INSERT INTO users (id, email, role) VALUES (?, ?, 'admin')")
                .bind(user)
                .bind(format!("{user}@example.com"))
                .execute(&pool)
                .await
                .unwrap();
        }
        // (key, owner)
        for (key, owner) in [
            ("k-parent", Some("u-maker")),
            ("k-child", None),
            ("k-grandchild", None),
            ("k-service", None),
            ("k-service-child", None),
            ("k-owned", Some("u-other")),
            ("k-orphan", None),
            ("k-loop-a", None),
            ("k-loop-b", None),
        ] {
            sqlx::query("INSERT INTO api_keys (id, name, key_hash, key_prefix, owner_user_id) VALUES (?, ?, ?, 'lg-x', ?)")
                .bind(key)
                .bind(key)
                .bind(format!("hash-{key}"))
                .bind(owner)
                .execute(&pool)
                .await
                .unwrap();
        }
        // (actor key, made key): who made what, as the audit log recorded it.
        for (n, (actor, target)) in [
            (Some("k-parent"), "k-child"),
            (Some("k-child"), "k-grandchild"),
            (None, "k-service"),
            (Some("k-service"), "k-service-child"),
            (Some("k-parent"), "k-owned"),
            (Some("k-gone"), "k-orphan"),
            (Some("k-loop-b"), "k-loop-a"),
            (Some("k-loop-a"), "k-loop-b"),
        ]
        .into_iter()
        .enumerate()
        {
            sqlx::query(
                "INSERT INTO audit_log (id, actor_key_id, actor_label, action, target_type, target_id) \
                 VALUES (?, ?, ?, 'key.create', 'api_key', ?)",
            )
            .bind(format!("audit-{n}"))
            .bind(actor)
            .bind(actor.unwrap_or("master-key"))
            .bind(target)
            .execute(&pool)
            .await
            .unwrap();
        }
        // A rotation is not a creation: it never names a maker.
        sqlx::query(
            "INSERT INTO audit_log (id, actor_key_id, actor_label, action, target_type, target_id) \
             VALUES ('audit-rotate', 'k-parent', 'k-parent', 'key.rotate', 'api_key', 'k-orphan')",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::migrate!("./migrations/sqlite").run(&pool).await.expect("migration 025 applies");

        let owners: std::collections::HashMap<String, Option<String>> =
            sqlx::query_as::<_, (String, Option<String>)>("SELECT id, owner_user_id FROM api_keys")
                .fetch_all(&pool)
                .await
                .unwrap()
                .into_iter()
                .collect();
        let owner = |key: &str| owners[key].as_deref().map(str::to_string);
        assert_eq!(owner("k-child").as_deref(), Some("u-maker"), "made by the maker's key");
        assert_eq!(owner("k-grandchild").as_deref(), Some("u-maker"), "made by a key the maker's key made");
        assert_eq!(owner("k-service"), None, "made with the master key: the org's");
        assert_eq!(owner("k-service-child"), None, "made by the org's key: the org's");
        assert_eq!(owner("k-owned").as_deref(), Some("u-other"), "an owner already set is kept");
        assert_eq!(owner("k-orphan"), None, "its maker's row is gone, and a rotation names no maker");
        assert_eq!((owner("k-loop-a"), owner("k-loop-b")), (None, None), "a cycle ends with no owner");
    }

    /// The audit rows a migration test reads back: id → (before_json, after_json).
    async fn audit_json_columns(pool: &sqlx::SqlitePool) -> std::collections::HashMap<String, (Option<String>, Option<String>)> {
        sqlx::query_as::<_, (String, Option<String>, Option<String>)>("SELECT id, before_json, after_json FROM audit_log")
            .fetch_all(pool)
            .await
            .unwrap()
            .into_iter()
            .map(|(id, before, after)| (id, (before, after)))
            .collect()
    }

    /// N1 (leftovers 2026-11-14): until then a `provider.configure` audit
    /// entry kept each extra header's value, a gateway's auth among them, and
    /// `GET /v1/audit` served it. Migration 027 writes `[redacted]` over every
    /// such value, in `before_json` and `after_json`, as entries are written
    /// now: the header names and the rest of the settings stay. Every other
    /// row is left byte for byte as it was, and running it again changes
    /// nothing.
    #[tokio::test]
    async fn migration_0027_redacts_provider_header_values_in_the_audit_log() {
        use sqlx::sqlite::SqlitePoolOptions;

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite:file:mig0027?mode=memory&cache=shared")
            .await
            .expect("connect fresh sqlite");
        let mut before = sqlx::migrate!("./migrations/sqlite");
        before.migrations =
            std::borrow::Cow::Owned(before.migrations.iter().filter(|m| m.version < 20240101000027).cloned().collect());
        before.run(&pool).await.expect("migrations before 027");

        // (id, action, before_json, after_json), as entries were written before 027.
        let rows = [
            ("a-headers", "provider.configure", None, Some(r#"{"api_base":"https://gateway.example/v1","extra_headers":{"cf-aig-authorization":"Bearer cf-token-1","x-team":"growth"}}"#)),
            ("a-before", "provider.configure", Some(r#"{"extra_headers":{"Helicone-Auth":"Bearer hk-1"}}"#), Some(r#"{"enabled":false}"#)),
            ("a-no-headers", "provider.configure", None, Some(r#"{"api_base":"https://other.example/v1"}"#)),
            ("a-empty", "provider.configure", None, Some(r#"{"extra_headers":{}}"#)),
            ("a-redacted", "provider.configure", None, Some(r#"{"extra_headers":{"x-team":"[redacted]"}}"#)),
            ("a-nothing", "provider.configure", None, None),
            ("a-not-json", "provider.configure", None, Some("not json {")),
            ("a-other-action", "settings.update", None, Some(r#"{"extra_headers":{"x-kept":"kept"}}"#)),
        ];
        for (id, action, before_json, after_json) in rows {
            sqlx::query(
                "INSERT INTO audit_log (id, actor_label, action, target_type, target_id, before_json, after_json) \
                 VALUES (?, 'master-key', ?, 'provider', 'openai', ?, ?)",
            )
            .bind(id)
            .bind(action)
            .bind(before_json)
            .bind(after_json)
            .execute(&pool)
            .await
            .unwrap();
        }

        let migrator = sqlx::migrate!("./migrations/sqlite");
        migrator.run(&pool).await.expect("migration 027 applies");
        let stored = audit_json_columns(&pool).await;
        let parsed = |text: &Option<String>| text.as_deref().map(|t| serde_json::from_str::<serde_json::Value>(t).expect("still JSON"));
        assert_eq!(
            parsed(&stored["a-headers"].1),
            Some(serde_json::json!({
                "api_base": "https://gateway.example/v1",
                "extra_headers": { "cf-aig-authorization": "[redacted]", "x-team": "[redacted]" },
            })),
            "after_json keeps the names and the rest of the settings"
        );
        assert_eq!(parsed(&stored["a-before"].0), Some(serde_json::json!({ "extra_headers": { "Helicone-Auth": "[redacted]" } })), "before_json too");
        for (id, _, before_json, after_json) in rows {
            let as_written = (before_json.map(str::to_string), after_json.map(str::to_string));
            match id {
                "a-headers" => {}
                "a-before" => assert_eq!(stored[id].1, as_written.1, "{id}: its after_json holds no headers"),
                _ => assert_eq!(stored[id], as_written, "{id} is left as it was"),
            }
        }

        let again = &migrator.migrations.iter().find(|m| m.version == 20240101000027).expect("migration 027").sql;
        sqlx::raw_sql(again).execute(&pool).await.expect("027 runs a second time");
        assert_eq!(audit_json_columns(&pool).await, stored, "running 027 again changes nothing");
    }

    /// N1 follow-up (2026-11-14): until then a `settings.update` audit entry
    /// kept the whole alert webhook URL, which can carry its secret in the
    /// path (Slack, Discord) or in the host (Pipedream, an AWS Lambda function
    /// URL). Migration 028 writes `[redacted]` over every such URL, in
    /// `before_json` and `after_json`, as entries are written now. An empty or
    /// null URL, every other row and text that isn't JSON are left byte for
    /// byte as they were, and running it again changes nothing.
    #[tokio::test]
    async fn migration_0028_redacts_alert_webhook_urls_in_the_audit_log() {
        use sqlx::sqlite::SqlitePoolOptions;

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite:file:mig0028?mode=memory&cache=shared")
            .await
            .expect("connect fresh sqlite");
        let mut before = sqlx::migrate!("./migrations/sqlite");
        before.migrations =
            std::borrow::Cow::Owned(before.migrations.iter().filter(|m| m.version < 20240101000028).cloned().collect());
        before.run(&pool).await.expect("migrations before 028");

        // (id, action, before_json, after_json), as entries were written before 028.
        let rows = [
            ("s-slack", "settings.update", None, Some(r#"{"alert_webhook_url":"https://hooks.slack.com/services/T0001/B0002/secret-1","cost_markup_percent":12.5}"#)),
            ("s-odd-case", "settings.update", None, Some(r#"{"alert_webhook_url":"HTTPS://User:Pass@Hooks.Example.COM:443/alerts?token=abc#frag"}"#)),
            ("s-port", "settings.update", None, Some(r#"{"alert_webhook_url":"http://alerts.internal.example:8080/hook?key=k"}"#)),
            ("s-default-port", "settings.update", None, Some(r#"{"alert_webhook_url":"http://10.0.0.7:80/x"}"#)),
            ("s-backslash", "settings.update", None, Some(r#"{"alert_webhook_url":"https://hooks.example.com\\services\\secret-3"}"#)),
            ("s-ipv6", "settings.update", None, Some(r#"{"alert_webhook_url":"https://[::1]:9000/secret-4"}"#)),
            ("s-host-secret", "settings.update", None, Some(r#"{"alert_webhook_url":"https://eo3secret77.m.pipedream.net/hook"}"#)),
            ("s-before", "settings.update", Some(r#"{"alert_webhook_url":"https://discord.com/api/webhooks/1/secret-2"}"#), Some(r#"{"alert_webhook_url":null}"#)),
            ("s-done", "settings.update", None, Some(r#"{"alert_webhook_url":"[redacted]"}"#)),
            ("s-empty", "settings.update", None, Some(r#"{"alert_webhook_url":""}"#)),
            ("s-no-url", "settings.update", None, Some(r#"{"cost_markup_percent":3}"#)),
            ("s-nothing", "settings.update", None, None),
            ("s-not-json", "settings.update", None, Some("not json {")),
            ("s-other-action", "key.update", None, Some(r#"{"alert_webhook_url":"https://hooks.slack.com/services/kept"}"#)),
        ];
        for (id, action, before_json, after_json) in rows {
            sqlx::query(
                "INSERT INTO audit_log (id, actor_label, action, target_type, target_id, before_json, after_json) \
                 VALUES (?, 'master-key', ?, 'settings', 'gateway', ?, ?)",
            )
            .bind(id)
            .bind(action)
            .bind(before_json)
            .bind(after_json)
            .execute(&pool)
            .await
            .unwrap();
        }

        let migrator = sqlx::migrate!("./migrations/sqlite");
        migrator.run(&pool).await.expect("migration 028 applies");
        let stored = audit_json_columns(&pool).await;
        let parsed = |text: &Option<String>| text.as_deref().map(|t| serde_json::from_str::<serde_json::Value>(t).expect("still JSON"));
        // The entries whose after_json 028 rewrites.
        let rewritten = ["s-slack", "s-odd-case", "s-port", "s-default-port", "s-backslash", "s-ipv6", "s-host-secret"];
        for id in rewritten {
            assert_eq!(parsed(&stored[id].1).expect("after_json")["alert_webhook_url"], "[redacted]", "{id}");
        }
        assert_eq!(parsed(&stored["s-slack"].1).expect("after_json")["cost_markup_percent"], 12.5, "the rest of the settings stay");
        assert_eq!(parsed(&stored["s-before"].0), Some(serde_json::json!({ "alert_webhook_url": "[redacted]" })), "before_json too");
        for (id, _, before_json, after_json) in rows {
            let as_written = (before_json.map(str::to_string), after_json.map(str::to_string));
            if rewritten.contains(&id) {
                assert_eq!(stored[id].0, as_written.0, "{id}: its before_json holds no URL");
            } else if id == "s-before" {
                assert_eq!(stored[id].1, as_written.1, "{id}: its after_json holds no URL");
            } else {
                assert_eq!(stored[id], as_written, "{id} is left as it was");
            }
        }

        let again = &migrator.migrations.iter().find(|m| m.version == 20240101000028).expect("migration 028").sql;
        sqlx::raw_sql(again).execute(&pool).await.expect("028 runs a second time");
        assert_eq!(audit_json_columns(&pool).await, stored, "running 028 again changes nothing");
    }

    /// Review 2026-11-15 round 2, finding 4: clearing a key's webhook stored
    /// `""`, which the outbox's trigger took for a webhook. Migration 032
    /// stores each of those as no webhook (NULL); every other key's
    /// webhook_url is left as it was.
    #[tokio::test]
    async fn migration_0032_stores_a_cleared_webhook_as_none() {
        use sqlx::sqlite::SqlitePoolOptions;

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite:file:mig0032?mode=memory&cache=shared")
            .await
            .expect("connect fresh sqlite");
        let mut before = sqlx::migrate!("./migrations/sqlite");
        before.migrations =
            std::borrow::Cow::Owned(before.migrations.iter().filter(|m| m.version < 20240101000032).cloned().collect());
        before.run(&pool).await.expect("migrations before 032");
        let keys = [("k-cleared", Some("")), ("k-hooked", Some("https://hooks.example/h")), ("k-none", None)];
        for (id, url) in keys {
            sqlx::query("INSERT INTO api_keys (id, name, key_hash, key_prefix, webhook_url) VALUES (?, ?, ?, 'lg-k', ?)")
                .bind(id)
                .bind(id)
                .bind(format!("hash-{id}"))
                .bind(url)
                .execute(&pool)
                .await
                .unwrap();
        }

        sqlx::migrate!("./migrations/sqlite").run(&pool).await.expect("migration 032 applies");
        let stored: Vec<(String, Option<String>)> = sqlx::query_as("SELECT id, webhook_url FROM api_keys ORDER BY id").fetch_all(&pool).await.unwrap();
        let expected = [("k-cleared", None), ("k-hooked", Some("https://hooks.example/h")), ("k-none", None)];
        assert_eq!(stored, expected.map(|(id, url)| (id.to_string(), url.map(str::to_string))), "only the cleared one changes");
    }

    /// Retention deleted generations' files but left the request log's copy
    /// of their output (an image's artifact keeps up to 2 MiB of it inline),
    /// and a cache hit after a delete wrote the image back under the deleted
    /// generation's id. Migration 040 clears the output of every artifact
    /// whose generation's files are deleted (`output_kind` `deleted`, no
    /// value), keeping its prompt and params. Every other artifact stays as
    /// it was: one whose files are kept, one with no output (a failed
    /// request's), and one with no generation row.
    #[tokio::test]
    async fn migration_0040_clears_the_request_logs_copies_of_deleted_files() {
        use sqlx::sqlite::SqlitePoolOptions;

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite:file:mig0040?mode=memory&cache=shared")
            .await
            .expect("connect fresh sqlite");
        let mut before = sqlx::migrate!("./migrations/sqlite");
        before.migrations =
            std::borrow::Cow::Owned(before.migrations.iter().filter(|m| m.version < 20240101000040).cloned().collect());
        before.run(&pool).await.expect("migrations before 040");
        for stmt in [
            "INSERT INTO generations (id, model, provider, media_type, status, files_deleted_at, files_deleted_reason) VALUES \
             ('g-swept', 'mock/image-gen', 'mock', 'image', 'completed', datetime('now'), 'retention'), \
             ('g-hit-after-delete', 'mock/image-gen', 'mock', 'image', 'completed', datetime('now'), 'deleted'), \
             ('g-video', 'mock/video-gen', 'mock', 'video', 'completed', datetime('now'), 'retention'), \
             ('g-kept', 'mock/image-gen', 'mock', 'image', 'completed', NULL, NULL), \
             ('g-failed', 'mock/image-gen', 'mock', 'image', 'failed', datetime('now'), 'retention')",
            "INSERT INTO request_artifacts (request_id, media_type, prompt, params_json, output_kind, output_value, output_mime, output_truncated) VALUES \
             ('g-swept', 'image', 'a red cube', '{\"size\":\"1024x1024\"}', 'b64', 'AAAA', 'image/png', 1), \
             ('g-hit-after-delete', 'image', 'a red cube', NULL, 'b64', 'BBBB', 'image/png', 0), \
             ('g-video', 'video', 'a wave', NULL, 'url', 'http://127.0.0.1:4000/v1/assets/litegen/video/g-video/video.mp4', 'video/mp4', 0), \
             ('g-kept', 'image', 'a kept cube', NULL, 'b64', 'CCCC', 'image/png', 0), \
             ('g-failed', 'image', 'a refused cube', NULL, 'error', NULL, NULL, 0), \
             ('litegen-img-no-row', 'image', 'a cube with no row', NULL, 'b64', 'DDDD', 'image/png', 0)",
        ] {
            sqlx::query(stmt).execute(&pool).await.unwrap();
        }

        sqlx::migrate!("./migrations/sqlite").run(&pool).await.expect("migration 040 applies");
        // request_id, output_kind, output_value, output_truncated, prompt, params_json
        type Row = (String, String, Option<String>, bool, Option<String>, Option<String>);
        let stored: Vec<Row> = sqlx::query_as(
            "SELECT request_id, output_kind, output_value, output_truncated, prompt, params_json FROM request_artifacts ORDER BY request_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let row = |id: &str, kind: &str, value: Option<&str>, truncated: bool, prompt: &str, params: Option<&str>| {
            (id.to_string(), kind.to_string(), value.map(str::to_string), truncated, Some(prompt.to_string()), params.map(str::to_string))
        };
        assert_eq!(
            stored,
            vec![
                row("g-failed", "error", None, false, "a refused cube", None),
                row("g-hit-after-delete", "deleted", None, false, "a red cube", None),
                row("g-kept", "b64", Some("CCCC"), false, "a kept cube", None),
                row("g-swept", "deleted", None, false, "a red cube", Some("{\"size\":\"1024x1024\"}")),
                row("g-video", "deleted", None, false, "a wave", None),
                row("litegen-img-no-row", "b64", Some("DDDD"), false, "a cube with no row", None),
            ],
            "only the outputs of generations whose files are deleted go"
        );
    }

    /// Deleting a generation (`DELETE /v1/generations/{id}`) removes its
    /// request from the request log, and with it the artifacts of the cache
    /// hits it answered: they name it (`output_kind` `generation`) and carry
    /// its prompt and params, the same request by construction. Another
    /// generation's hits, and every other artifact, stay.
    #[tokio::test]
    async fn redacting_a_generation_deletes_the_artifacts_of_its_cache_hits() {
        let db = in_memory_db().await;
        db.insert_generation("g-deleted", None, "mock/image-gen", "mock", "image", None, 0.0, None, None).await.unwrap();
        let ids = [
            ("g-deleted", "b64", "AAAA"),
            ("hit-1", "generation", "g-deleted"),
            ("hit-2", "generation", "g-deleted"),
            ("hit-of-another", "generation", "g-kept"),
            ("g-kept", "b64", "BBBB"),
            ("g-deleted-named-in-a-url", "url", "g-deleted"),
        ];
        for (id, kind, value) in ids {
            db.insert_request_artifact(&RequestArtifact {
                request_id: id.into(),
                media_type: "image".into(),
                prompt: Some("a red cube".into()),
                negative_prompt: None,
                params_json: None,
                refs_meta_json: None,
                output_kind: kind.into(),
                output_value: Some(value.into()),
                output_mime: Some("image/png".into()),
                output_truncated: false,
                error_message: None,
                created_at: chrono::Utc::now(),
                org_id: None,
                app_id: None,
            })
            .await
            .unwrap();
        }

        db.redact_generation_request("g-deleted").await.unwrap();
        let mut left = Vec::new();
        for (id, ..) in ids {
            if db.get_request_artifact(id).await.unwrap().is_some() {
                left.push(id);
            }
        }
        assert_eq!(left, ["hit-of-another", "g-kept", "g-deleted-named-in-a-url"], "its own artifact and its hits' go");
    }

    /// A rotation records who holds the new secret in the same statement that
    /// sets it; with no new owner the key keeps the one it had.
    #[tokio::test]
    async fn rotate_api_key_takes_a_new_owner_or_keeps_the_old() {
        let db = in_memory_db().await;
        let now = chrono::Utc::now();
        for id in ["u-first", "u-second"] {
            let user = User {
                id: id.to_string(),
                email: format!("{id}@example.com"),
                password_hash: None,
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
        }
        let key = db.create_api_key("k", "hash-0", "lg-0", None, None, "generate", None).await.unwrap();
        db.set_api_key_owner(&key.id, "u-first").await.unwrap();

        let kept = db.rotate_api_key(&key.id, "pk_1", "hash-1", "lg-1", None).await.unwrap().expect("rotated");
        assert_eq!((kept.key_hash.as_str(), kept.owner_user_id.as_deref()), ("hash-1", Some("u-first")));
        let taken = db.rotate_api_key(&key.id, "pk_2", "hash-2", "lg-2", Some("u-second")).await.unwrap().expect("rotated");
        assert_eq!((taken.key_hash.as_str(), taken.owner_user_id.as_deref()), ("hash-2", Some("u-second")));
        assert!(db.rotate_api_key(&Uuid::new_v4(), "pk_3", "hash-3", "lg-3", Some("u-second")).await.unwrap().is_none());
    }

    // ─── Tenancy CRUD tests ───────────────────────────────────────────────────

    use crate::types::{Application, Organization};

    fn make_org(id: &str, slug: &str) -> Organization {
        let now = chrono::Utc::now();
        Organization {
            id: id.to_string(),
            name: format!("Org {slug}"),
            slug: slug.to_string(),
            plan: "free".to_string(),
            status: "active".to_string(),
            created_at: now,
            updated_at: now,
        }
    }

    fn make_app(id: &str, org_id: &str, slug: &str) -> Application {
        let now = chrono::Utc::now();
        Application {
            id: id.to_string(),
            org_id: org_id.to_string(),
            name: format!("App {slug}"),
            slug: slug.to_string(),
            status: "active".to_string(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Security fix 3 (2026-11-05): the single-tenant Owner lives in
    /// `users.role` and in the default org's membership. A transfer moves
    /// both, or neither.
    #[tokio::test]
    async fn transfer_deployment_owner_moves_both_places_or_neither() {
        let db = in_memory_db().await;
        let org = make_org(&Uuid::new_v4().to_string(), "deploy");
        db.create_organization(&org).await.unwrap();
        let owner = make_user("deploy-owner@example.com", Role::Owner);
        let admin = make_user("deploy-admin@example.com", Role::Admin);
        let outsider = make_user("deploy-outsider@example.com", Role::Member);
        for u in [&owner, &admin, &outsider] {
            db.create_user(u).await.unwrap();
        }
        db.add_org_member(&org.id, &owner.id, Role::Owner).await.unwrap();
        db.add_org_member(&org.id, &admin.id, Role::Admin).await.unwrap();
        let account = |id: String| {
            let db = &db;
            async move { db.get_user_by_id(&id).await.unwrap().unwrap().role }
        };

        // Someone with no membership can't take it, and nothing moves.
        assert!(matches!(db.transfer_deployment_owner(&org.id, &outsider.id).await, Err(sqlx::Error::RowNotFound)));
        assert_eq!(db.get_membership(&org.id, &owner.id).await.unwrap(), Some(Role::Owner));
        assert_eq!(account(owner.id.clone()).await, Role::Owner);
        assert_eq!(account(outsider.id.clone()).await, Role::Member);

        // A member takes it, in both places.
        db.transfer_deployment_owner(&org.id, &admin.id).await.unwrap();
        assert_eq!(db.get_membership(&org.id, &admin.id).await.unwrap(), Some(Role::Owner));
        assert_eq!(db.get_membership(&org.id, &owner.id).await.unwrap(), Some(Role::Admin));
        assert_eq!(account(admin.id.clone()).await, Role::Owner);
        assert_eq!(account(owner.id.clone()).await, Role::Admin);
    }

    #[tokio::test]
    async fn org_app_member_crud() {
        let db = in_memory_db().await;

        // Create an org and an app within it.
        let org = make_org(&Uuid::new_v4().to_string(), "acme");
        db.create_organization(&org).await.unwrap();
        let got_org = db.get_organization(&org.id).await.unwrap().expect("org exists");
        assert_eq!(got_org.slug, "acme");
        assert_eq!(got_org.plan, "free");
        let by_slug = db.get_org_by_slug("acme").await.unwrap().expect("by slug");
        assert_eq!(by_slug.id, org.id);

        let app = make_app(&Uuid::new_v4().to_string(), &org.id, "web");
        db.create_application(&app).await.unwrap();
        let got_app = db.get_application(&app.id).await.unwrap().expect("app exists");
        assert_eq!(got_app.org_id, org.id);
        assert_eq!(got_app.slug, "web");
        let apps = db.list_apps_for_org(&org.id).await.unwrap();
        assert_eq!(apps.len(), 1);

        // Create two users and add them as members.
        let owner = make_user("orgowner@example.com", Role::Owner);
        let admin = make_user("orgadmin@example.com", Role::Member);
        db.create_user(&owner).await.unwrap();
        db.create_user(&admin).await.unwrap();
        db.add_org_member(&org.id, &owner.id, Role::Owner).await.unwrap();
        db.add_org_member(&org.id, &admin.id, Role::Admin).await.unwrap();

        // get_membership returns the role.
        assert_eq!(db.get_membership(&org.id, &owner.id).await.unwrap(), Some(Role::Owner));
        assert_eq!(db.get_membership(&org.id, &admin.id).await.unwrap(), Some(Role::Admin));
        assert_eq!(db.get_membership(&org.id, "nobody").await.unwrap(), None);

        // list_org_members includes the joined email.
        let members = db.list_org_members(&org.id).await.unwrap();
        assert_eq!(members.len(), 2);
        assert!(members.iter().any(|m| m.email == "orgowner@example.com" && m.role == Role::Owner));
        assert!(members.iter().any(|m| m.email == "orgadmin@example.com" && m.role == Role::Admin));

        // update_member_role.
        db.update_member_role(&org.id, &admin.id, Role::Viewer).await.unwrap();
        assert_eq!(db.get_membership(&org.id, &admin.id).await.unwrap(), Some(Role::Viewer));

        // list_orgs_for_user returns the org + role.
        let orgs = db.list_orgs_for_user(&owner.id).await.unwrap();
        assert_eq!(orgs.len(), 1);
        assert_eq!(orgs[0].0.id, org.id);
        assert_eq!(orgs[0].1, Role::Owner);

        // transfer_org_owner swaps roles: owner -> admin, admin(now viewer) -> owner.
        db.transfer_org_owner(&org.id, &admin.id).await.unwrap();
        assert_eq!(db.get_membership(&org.id, &owner.id).await.unwrap(), Some(Role::Admin));
        assert_eq!(db.get_membership(&org.id, &admin.id).await.unwrap(), Some(Role::Owner));

        // transfer_org_owner to a non-member must fail and roll back the demote,
        // leaving the current owner intact (the org never becomes ownerless).
        assert!(db.transfer_org_owner(&org.id, "not-a-member").await.is_err());
        assert_eq!(db.get_membership(&org.id, &admin.id).await.unwrap(), Some(Role::Owner));
        assert_eq!(db.get_membership(&org.id, &owner.id).await.unwrap(), Some(Role::Admin));

        // update_organization renames.
        let renamed = db.update_organization(&org.id, Some("Renamed Inc")).await.unwrap().unwrap();
        assert_eq!(renamed.name, "Renamed Inc");

        // remove_org_member.
        db.remove_org_member(&org.id, &owner.id).await.unwrap();
        assert_eq!(db.get_membership(&org.id, &owner.id).await.unwrap(), None);

        // delete_application removes the app.
        assert!(db.delete_application(&app.id).await.unwrap());
        assert!(db.get_application(&app.id).await.unwrap().is_none());

        // delete_organization removes a (fresh, empty) org and reports success.
        let empty_org = make_org(&Uuid::new_v4().to_string(), "to-delete");
        db.create_organization(&empty_org).await.unwrap();
        assert!(db.delete_organization(&empty_org.id).await.unwrap());
        assert!(db.get_organization(&empty_org.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn tenant_generation_isolation() {
        let db = in_memory_db().await;

        let org_a = make_org(&Uuid::new_v4().to_string(), "org-a");
        let org_b = make_org(&Uuid::new_v4().to_string(), "org-b");
        db.create_organization(&org_a).await.unwrap();
        db.create_organization(&org_b).await.unwrap();
        let app_a = make_app(&Uuid::new_v4().to_string(), &org_a.id, "app-a");
        let app_b = make_app(&Uuid::new_v4().to_string(), &org_b.id, "app-b");
        db.create_application(&app_a).await.unwrap();
        db.create_application(&app_b).await.unwrap();

        // Insert a generation row per tenant, stamped directly via insert_generation.
        db.insert_generation("gen-a", None, "mock/v", "mock", "video", None, 0.0, Some(&org_a.id), Some(&app_a.id)).await.unwrap();
        db.insert_generation("gen-b", None, "mock/v", "mock", "video", None, 0.0, Some(&org_b.id), Some(&app_b.id)).await.unwrap();

        // org A + app A sees only A's row.
        let rows = db.list_generations_for_tenant(&org_a.id, Some(&app_a.id), 1, 50).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "gen-a");
        assert_eq!(db.count_generations_for_tenant(&org_a.id, Some(&app_a.id)).await.unwrap(), 1);

        // org A without app filter still only sees A's row (never B's).
        let rows = db.list_generations_for_tenant(&org_a.id, None, 1, 50).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "gen-a");
        assert!(rows.iter().all(|g| g.id != "gen-b"));

        // org B sees only B's row.
        let rows = db.list_generations_for_tenant(&org_b.id, None, 1, 50).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "gen-b");
        assert_eq!(db.count_generations_for_tenant(&org_b.id, None).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn create_api_key_scoped_persists_tenant() {
        let db = in_memory_db().await;
        let org = make_org(&Uuid::new_v4().to_string(), "key-org");
        db.create_organization(&org).await.unwrap();
        let app = make_app(&Uuid::new_v4().to_string(), &org.id, "key-app");
        db.create_application(&app).await.unwrap();

        let key = db.create_api_key_scoped(
            &org.id, &app.id, "pk_live_abc123", "scoped-key",
            "hash_scoped", "lg-sc", Some(25.0), Some(120), "generate,read", None, None,
        ).await.unwrap();
        assert_eq!(key.org_id.as_deref(), Some(org.id.as_str()));
        assert_eq!(key.app_id.as_deref(), Some(app.id.as_str()));
        assert_eq!(key.public_id.as_deref(), Some("pk_live_abc123"));
        assert_eq!(key.token_quota, Some(25.0));
        assert_eq!(key.rpm_limit, Some(120));

        // get_api_key shows the tenant fields populated.
        let fetched = db.get_api_key(&key.id).await.unwrap().expect("key");
        assert_eq!(fetched.org_id.as_deref(), Some(org.id.as_str()));
        assert_eq!(fetched.app_id.as_deref(), Some(app.id.as_str()));
        assert_eq!(fetched.public_id.as_deref(), Some("pk_live_abc123"));
        assert!(fetched.is_active);

        // list_api_keys_for_app returns it.
        let app_keys = db.list_api_keys_for_app(&app.id).await.unwrap();
        assert_eq!(app_keys.len(), 1);
        assert_eq!(app_keys[0].id, key.id);
    }

    #[tokio::test]
    async fn artifact_is_isolated_per_tenant() {
        let db = in_memory_db().await;

        // Set up two distinct orgs/apps.
        let org_a = make_org(&Uuid::new_v4().to_string(), "art-org-a");
        let org_b = make_org(&Uuid::new_v4().to_string(), "art-org-b");
        db.create_organization(&org_a).await.unwrap();
        db.create_organization(&org_b).await.unwrap();
        let app_a = make_app(&Uuid::new_v4().to_string(), &org_a.id, "art-app-a");
        let app_b = make_app(&Uuid::new_v4().to_string(), &org_b.id, "art-app-b");
        db.create_application(&app_a).await.unwrap();
        db.create_application(&app_b).await.unwrap();

        // Insert an artifact scoped to org A / app A.
        let artifact_a = RequestArtifact {
            request_id: "req-art-a".to_string(),
            media_type: "image".to_string(),
            prompt: Some("tenant A image".to_string()),
            negative_prompt: None,
            params_json: None,
            refs_meta_json: None,
            output_kind: "b64".to_string(),
            output_value: Some("abc123".to_string()),
            output_mime: Some("image/png".to_string()),
            output_truncated: false,
            error_message: None,
            created_at: chrono::Utc::now(),
            org_id: Some(org_a.id.clone()),
            app_id: Some(app_a.id.clone()),
        };
        db.insert_request_artifact(&artifact_a).await.unwrap();

        // App A's key can retrieve the artifact and it carries the correct org.
        let fetched = db.get_request_artifact("req-art-a").await.unwrap().expect("should exist");
        assert_eq!(fetched.org_id.as_deref(), Some(org_a.id.as_str()));

        // Simulate the get_log_artifact handler's cross-tenant check: org B cannot see it.
        let caller_org_b = org_b.id.as_str();
        let denied = fetched.org_id.as_deref().map(|o| o != caller_org_b).unwrap_or(false);
        assert!(denied, "org B should be denied access to org A's artifact");

        // Simulate org A accessing their own artifact — allowed.
        let caller_org_a = org_a.id.as_str();
        let allowed = !fetched.org_id.as_deref().map(|o| o != caller_org_a).unwrap_or(false);
        assert!(allowed, "org A should be allowed to access their own artifact");
    }

    // ─── Stats on an empty DB / tenant ────────────────────────────────────────

    /// Regression: `get_stats` / `get_stats_for_tenant` over a DB/tenant with NO
    /// request_logs must return zeroes, not a SQLx decode error. The success/failed
    /// columns use SUM(CASE ...), which returns NULL over an empty row set; without
    /// a COALESCE that NULL fails to decode into i64 ("error occurred while decoding
    /// column 1: unexpected null"). This is the crash a freshly-created org hit on
    /// the dashboard Overview page right after logging in.
    #[tokio::test]
    async fn stats_on_empty_db_and_tenant_returns_zeroes() {
        let db = in_memory_db().await;

        // Global stats over an empty request_logs table.
        let global = db.get_stats().await.expect("get_stats must not error on empty table");
        assert_eq!(global.total_requests, 0);
        assert_eq!(global.successful_requests, 0);
        assert_eq!(global.failed_requests, 0);
        assert_eq!(global.total_cost_usd, 0.0);

        // Tenant-scoped stats for a brand-new org/app with no logs (the Overview path).
        let org = make_org(&Uuid::new_v4().to_string(), "empty-stats");
        db.create_organization(&org).await.unwrap();
        let app = make_app(&Uuid::new_v4().to_string(), &org.id, "empty-app");
        db.create_application(&app).await.unwrap();

        let by_org = db
            .get_stats_for_tenant(&org.id, None, &[])
            .await
            .expect("get_stats_for_tenant (org only) must not error with no logs");
        assert_eq!(by_org.total_requests, 0);
        assert_eq!(by_org.successful_requests, 0);
        assert_eq!(by_org.failed_requests, 0);

        let by_app = db
            .get_stats_for_tenant(&org.id, Some(&app.id), &[])
            .await
            .expect("get_stats_for_tenant (org+app) must not error with no logs");
        assert_eq!(by_app.total_requests, 0);
        assert_eq!(by_app.successful_requests, 0);
        assert_eq!(by_app.failed_requests, 0);
    }

    #[tokio::test]
    async fn mark_invitation_used_is_atomic_single_use() {
        let db = in_memory_db().await;
        let token = generate_session_token();
        let inv = Invitation {
            id: Uuid::new_v4().to_string(),
            email: "alice@x.com".into(),
            role: Role::Member,
            token: token.clone(),
            invited_by: None,
            org_id: crate::api::middleware::DEFAULT_ORG_ID.to_string(),
            expires_at: chrono::Utc::now() + chrono::Duration::days(7),
            used_at: None,
            created_at: chrono::Utc::now(),
        };
        db.create_invitation(&inv).await.unwrap();

        assert!(db.mark_invitation_used(&token).await.unwrap(), "first consume must win");
        let after = db.get_invitation(&token).await.unwrap().unwrap();
        assert!(after.used_at.is_some(), "used_at should be set after first mark_invitation_used");
        assert!(!db.mark_invitation_used(&token).await.unwrap(), "second consume must lose (already used)");
    }

    // ─── Credential resolution round-trip ────────────────────────────────────

    /// Encrypts a sample credential, stores it via `upsert_org_provider_credential`,
    /// retrieves it with `get_org_provider_credential`, decrypts, parses with
    /// `ProviderCredentials::from_json`, and asserts the round-trip is lossless.
    #[tokio::test]
    async fn org_provider_credential_encrypt_store_decrypt_roundtrip() {
        let db = in_memory_db().await;
        let secrets_key: [u8; 32] = [7u8; 32];

        // Create an org so the FK constraint is satisfied.
        let org = make_org(&uuid::Uuid::new_v4().to_string(), "rt-org");
        db.create_organization(&org).await.expect("create org for FK");
        let org_id = org.id.clone();

        // Build a sample credentials JSON value (bearer key shape).
        let creds_value = serde_json::json!({ "api_key": "sk-roundtrip-secret-9876" });
        let plaintext = serde_json::to_vec(&creds_value).expect("serialize creds");

        // Encrypt.
        let (ciphertext, nonce) = crate::auth::secrets::encrypt(&secrets_key, &plaintext)
            .expect("encrypt must succeed");

        // Store via DB.
        db.upsert_org_provider_credential(&org_id, "openai", &ciphertext, &nonce, Some("…9876"))
            .await
            .expect("upsert must succeed");

        // Retrieve.
        let row = db
            .get_org_provider_credential(&org_id, "openai")
            .await
            .expect("get must succeed")
            .expect("row must exist");

        // Decrypt and parse.
        let decrypted = crate::auth::secrets::decrypt(&secrets_key, &row.0, &row.1)
            .expect("decrypt must succeed");
        let val: serde_json::Value =
            serde_json::from_slice(&decrypted).expect("decrypted bytes must be valid JSON");
        let parsed = crate::providers::ProviderCredentials::from_json(&val);

        // Assert: the api_key survives the round-trip.
        assert_eq!(
            parsed.api_key.as_deref(),
            Some("sk-roundtrip-secret-9876"),
            "api_key must survive encrypt→store→retrieve→decrypt→parse"
        );

        // Assert: no other provider's cred leaks into this one.
        assert!(
            db.get_org_provider_credential(&org_id, "stability")
                .await
                .unwrap()
                .is_none(),
            "a different provider must not return the stored credential"
        );
    }

    #[tokio::test]
    async fn get_request_logs_for_tenant_filtered_pushes_filters_to_db() {
        use crate::db::RequestLogFilter;
        let db = in_memory_db().await;
        let org = "flt-org";

        // 2 openai logs + 1 fal log, all in the same org.
        db.log_request("lg-1", "m", "openai", "completed", "image", 0.0, 10, None, None, Some(org), None).await.unwrap();
        db.log_request("lg-2", "m", "openai", "completed", "image", 0.0, 10, None, None, Some(org), None).await.unwrap();
        db.log_request("lg-3", "m", "fal", "completed", "image", 0.0, 10, None, None, Some(org), None).await.unwrap();

        // Filter by provider — count + rows reflect the true match, at the DB.
        let filter = RequestLogFilter { provider: Some("openai"), ..Default::default() };
        let (rows, total) = db.get_request_logs_for_tenant_filtered(org, None, &filter, 1, 50).await.unwrap();
        assert_eq!(total, 2, "provider filter must be applied + counted at the DB");
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|l| l.provider == "openai"));

        // Backdate lg-3 (fal) and use a `from` cutoff that excludes it.
        sqlx::query("UPDATE request_logs SET created_at = '2000-01-01 00:00:00' WHERE id = 'lg-3'")
            .execute(db.pool()).await.unwrap();
        let cutoff = chrono::DateTime::parse_from_rfc3339("2010-01-01T00:00:00Z").unwrap().with_timezone(&chrono::Utc);
        let filter = RequestLogFilter { from: Some(cutoff), ..Default::default() };
        let (_, total_after) = db.get_request_logs_for_tenant_filtered(org, None, &filter, 1, 50).await.unwrap();
        assert_eq!(total_after, 2, "the from-cutoff must exclude the backdated row via a DB timestamp comparison");
    }

    #[tokio::test]
    async fn list_generations_for_owner_filters_and_counts_at_db() {
        // A read-own member must see their complete history via a DB-level
        // ownership filter + count — not an in-memory filter over a truncated
        // window (which silently drops older rows and mis-reports the total).
        let db = in_memory_db().await;
        let org = make_org(&Uuid::new_v4().to_string(), "own-org");
        db.create_organization(&org).await.unwrap();
        let app = make_app(&Uuid::new_v4().to_string(), &org.id, "own-app");
        db.create_application(&app).await.unwrap();

        let key_a = db
            .create_api_key_scoped(&org.id, &app.id, "pk_a", "ka", "hash_a", "lg-a", None, None, "generate", None, None)
            .await
            .unwrap();
        let key_b = db
            .create_api_key_scoped(&org.id, &app.id, "pk_b", "kb", "hash_b", "lg-b", None, None, "generate", None, None)
            .await
            .unwrap();

        // 3 generations for A, 2 for B.
        for i in 0..3 {
            db.insert_generation(&format!("gen-a-{i}"), Some(&key_a.id), "mock/v", "mock", "video", None, 0.0, Some(&org.id), Some(&app.id)).await.unwrap();
        }
        for i in 0..2 {
            db.insert_generation(&format!("gen-b-{i}"), Some(&key_b.id), "mock/v", "mock", "video", None, 0.0, Some(&org.id), Some(&app.id)).await.unwrap();
        }

        let (rows, total) = db
            .list_generations_for_owner(&org.id, None, &[key_a.id], 1, 50)
            .await
            .unwrap();
        assert_eq!(total, 3, "total must count only A's generations at the DB layer");
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|g| g.key_id == Some(key_a.id)), "must return only A's rows");

        // Pagination is applied at the DB, and total still reflects the full match.
        let (page2, total2) = db
            .list_generations_for_owner(&org.id, None, &[key_a.id], 2, 2)
            .await
            .unwrap();
        assert_eq!(total2, 3);
        assert_eq!(page2.len(), 1, "page 2 of size 2 over 3 rows holds the last row");

        // No owned keys => nothing.
        let (empty, zero) = db.list_generations_for_owner(&org.id, None, &[], 1, 50).await.unwrap();
        assert!(empty.is_empty() && zero == 0);
    }

    #[tokio::test]
    async fn reserve_tokens_enforces_quota_atomically() {
        // reserve_tokens is the real quota gate: it must reject a reservation
        // that would push tokens_used past token_quota, and must NOT increment
        // when it rejects (so the next request sees the true remaining quota).
        let db = in_memory_db().await;
        let org = make_org(&Uuid::new_v4().to_string(), "q-org");
        db.create_organization(&org).await.unwrap();
        let app = make_app(&Uuid::new_v4().to_string(), &org.id, "q-app");
        db.create_application(&app).await.unwrap();

        // Key with a $10 quota.
        let key = db
            .create_api_key_scoped(
                &org.id, &app.id, "pk_q", "quota-key", "hash_q", "lg-q",
                Some(10.0), None, "generate", None, None,
            )
            .await
            .unwrap();

        assert!(db.reserve_tokens(&key.id, 9.0).await.unwrap(), "9 <= 10 must reserve");
        assert!(
            !db.reserve_tokens(&key.id, 5.0).await.unwrap(),
            "9 + 5 > 10 must be rejected"
        );
        let after = db.get_api_key(&key.id).await.unwrap().unwrap();
        assert!(
            (after.tokens_used - 9.0).abs() < 1e-9,
            "a rejected reservation must not change tokens_used, got {}",
            after.tokens_used
        );
        assert!(
            db.reserve_tokens(&key.id, 1.0).await.unwrap(),
            "9 + 1 == 10 must reserve (boundary is inclusive)"
        );

        // A key with no quota reserves any amount.
        let unlimited = db
            .create_api_key_scoped(
                &org.id, &app.id, "pk_u", "unlimited", "hash_u", "lg-u",
                None, None, "generate", None, None,
            )
            .await
            .unwrap();
        assert!(
            db.reserve_tokens(&unlimited.id, 1_000_000.0).await.unwrap(),
            "a key with no quota reserves any amount"
        );
    }

    #[tokio::test]
    async fn create_api_key_scoped_persists_expiry() {
        // expires_at is enforced at auth (middleware) and settable on PATCH, so
        // it must also be persisted at creation — otherwise an SDK caller who
        // asks for a short-lived key silently gets one that never expires.
        let db = in_memory_db().await;
        let org = make_org(&Uuid::new_v4().to_string(), "exp-org");
        db.create_organization(&org).await.unwrap();
        let app = make_app(&Uuid::new_v4().to_string(), &org.id, "exp-app");
        db.create_application(&app).await.unwrap();

        let expiry = chrono::Utc::now() + chrono::Duration::hours(24);
        let key = db
            .create_api_key_scoped(
                &org.id, &app.id, "pk_live_exp", "ephemeral", "hash_exp", "lg-exp",
                None, None, "generate", None, Some(expiry),
            )
            .await
            .unwrap();

        // Returned key and the persisted row both carry the expiry.
        assert!(key.expires_at.is_some(), "created key must carry the requested expiry");
        let fetched = db.get_api_key(&key.id).await.unwrap().expect("key");
        let stored = fetched.expires_at.expect("expiry persisted");
        assert!(
            (stored - expiry).num_seconds().abs() <= 1,
            "persisted expiry {stored} must match requested {expiry}"
        );

        // A key created without an expiry stays non-expiring.
        let perpetual = db
            .create_api_key_scoped(
                &org.id, &app.id, "pk_live_perp", "perpetual", "hash_perp", "lg-perp",
                None, None, "generate", None, None,
            )
            .await
            .unwrap();
        assert!(perpetual.expires_at.is_none(), "no expiry requested => never expires");
    }

    #[tokio::test]
    async fn delete_application_cascades_child_rows() {
        // Deleting an app must remove the rows that reference it. On Postgres
        // those columns carry FKs without ON DELETE CASCADE, so a bare
        // `DELETE FROM applications` errors once the app has been used and the
        // app becomes permanently undeletable. The fix cascades in a
        // transaction, mirroring delete_organization. On SQLite (FKs off in
        // tests) a bare delete instead orphans the child rows, so this test
        // asserts the cascade semantics directly: no orphans left behind.
        let db = in_memory_db().await;
        let org = make_org(&Uuid::new_v4().to_string(), "del-org");
        db.create_organization(&org).await.unwrap();
        let app = make_app(&Uuid::new_v4().to_string(), &org.id, "del-app");
        db.create_application(&app).await.unwrap();

        // Use the app: issue a key and record a generation scoped to it.
        let key = db
            .create_api_key_scoped(
                &org.id, &app.id, "pk_live_del", "k", "hash_del", "lg-del",
                None, None, "generate", None, None,
            )
            .await
            .unwrap();
        db.insert_generation(
            "gen-del-1", Some(&key.id), "mock/v", "mock", "video", None, 0.0,
            Some(&org.id), Some(&app.id),
        )
        .await
        .unwrap();

        // A used app must still be deletable...
        let deleted = db.delete_application(&app.id).await.unwrap();
        assert!(deleted, "delete_application must report success for a used app");

        // ...and must not leave orphaned child rows behind.
        assert!(
            db.list_api_keys_for_app(&app.id).await.unwrap().is_empty(),
            "api_keys referencing the deleted app must be removed"
        );
        assert!(
            db.get_generation("gen-del-1").await.unwrap().is_none(),
            "generations referencing the deleted app must be removed"
        );
    }

    // ─── merge-on-write metadata and stage claims ─────────────────────────

    async fn active_row(db: &SqliteDatabase, id: &str) {
        db.insert_generation(id, None, "mock/mesh-3d", "mock", "model3d", Some("job"), 0.0, None, None).await.unwrap();
    }

    async fn metadata_of(db: &SqliteDatabase, id: &str) -> serde_json::Value {
        db.get_generation(id).await.unwrap().unwrap().metadata.unwrap_or(serde_json::Value::Null)
    }

    #[tokio::test]
    async fn a_terminal_write_keeps_what_submit_recorded() {
        let db = in_memory_db().await;
        active_row(&db, "g-merge").await;
        db.merge_generation_metadata_if_active("g-merge", &serde_json::json!({
            "request": { "prompt": "a red chair" },
            "requested_formats": ["glb"],
        })).await.unwrap();
        let w = db.update_generation_if_active(
            "g-merge", "completed", 100, Some("https://x/model.glb"), None, Some(chrono::Utc::now()),
            Some(&serde_json::json!({ "assets": [{ "kind": "mesh" }] })),
        ).await.unwrap();
        assert!(w.owns_outcome());
        let m = metadata_of(&db, "g-merge").await;
        assert_eq!(m["request"]["prompt"], "a red chair", "{m}");
        assert_eq!(m["requested_formats"], serde_json::json!(["glb"]), "{m}");
        assert_eq!(m["assets"][0]["kind"], "mesh", "{m}");
    }

    #[tokio::test]
    async fn a_patch_replaces_top_level_keys_whole_and_null_removes_one() {
        let db = in_memory_db().await;
        active_row(&db, "g-patch").await;
        db.merge_generation_metadata_if_active("g-patch", &serde_json::json!({
            "stage_context": { "a": 1, "b": 2 }, "keep": true,
        })).await.unwrap();
        db.merge_generation_metadata_if_active("g-patch", &serde_json::json!({
            "stage_context": { "a": 3 }, "keep": null,
        })).await.unwrap();
        let m = metadata_of(&db, "g-patch").await;
        assert_eq!(m["stage_context"], serde_json::json!({ "a": 3 }), "a nested object is replaced, not merged: {m}");
        assert!(m.get("keep").is_none(), "{m}");
    }

    #[tokio::test]
    async fn a_merge_never_touches_a_terminal_row() {
        let db = in_memory_db().await;
        active_row(&db, "g-done").await;
        db.update_generation_if_active("g-done", "failed", 0, None, Some("boom"), Some(chrono::Utc::now()), None).await.unwrap();
        let w = db.merge_generation_metadata_if_active("g-done", &serde_json::json!({ "late": 1 })).await.unwrap();
        assert_eq!(w, crate::db::GenerationWrite::AlreadyTerminal);
        assert!(metadata_of(&db, "g-done").await.get("late").is_none());
        let missing = db.merge_generation_metadata_if_active("g-nope", &serde_json::json!({ "x": 1 })).await.unwrap();
        assert_eq!(missing, crate::db::GenerationWrite::Missing);
    }

    #[tokio::test]
    async fn a_stage_is_claimed_once() {
        let db = in_memory_db().await;
        active_row(&db, "g-claim").await;
        assert!(db.claim_generation_stage("g-claim", "rig", "poller").await.unwrap());
        assert!(!db.claim_generation_stage("g-claim", "rig", "client").await.unwrap(), "a second claimant loses");
        assert!(db.claim_generation_stage("g-claim", "refine", "client").await.unwrap(), "another stage is its own claim");
        let m = metadata_of(&db, "g-claim").await;
        assert_eq!(m["stage_claims"]["rig"]["by"], "poller", "{m}");
        assert!(m["stage_claims"]["rig"]["at"].is_string(), "{m}");
    }

    #[tokio::test]
    async fn racing_claimants_produce_exactly_one_winner() {
        let db = std::sync::Arc::new(in_memory_db().await);
        active_row(&db, "g-race").await;
        let mut tasks = Vec::new();
        for i in 0..8 {
            let db = db.clone();
            tasks.push(tokio::spawn(async move { db.claim_generation_stage("g-race", "rig", &format!("o{i}")).await.unwrap() }));
        }
        let mut wins = 0;
        for t in tasks {
            if t.await.unwrap() { wins += 1; }
        }
        assert_eq!(wins, 1);
    }

    #[tokio::test]
    async fn a_terminal_row_cannot_be_claimed() {
        let db = in_memory_db().await;
        active_row(&db, "g-term").await;
        db.update_generation_if_active("g-term", "cancelled", 0, None, None, Some(chrono::Utc::now()), None).await.unwrap();
        assert!(!db.claim_generation_stage("g-term", "rig", "poller").await.unwrap());
    }

    /// The poller's skip warning (migration 035): each 10-minute mark of a
    /// row's age is noted once, a later mark again, never an earlier one, and
    /// a row no longer in flight never.
    #[tokio::test]
    async fn a_skip_warning_is_noted_once_per_mark() {
        let db = in_memory_db().await;
        active_row(&db, "g-skip").await;
        let noted = |mark| db.note_poll_skip_warning("g-skip", mark);
        assert!(noted(0).await.unwrap(), "the first skip warns");
        assert!(!noted(0).await.unwrap(), "the same mark again");
        assert!(noted(2).await.unwrap(), "a later mark");
        assert!(!noted(1).await.unwrap(), "an earlier mark than the last warned");
        assert!(!noted(2).await.unwrap());
        db.update_generation_if_active("g-skip", "cancelled", 0, None, None, Some(chrono::Utc::now()), None).await.unwrap();
        assert!(!noted(3).await.unwrap(), "a row that ended");
        assert!(!db.note_poll_skip_warning("g-nobody", 0).await.unwrap(), "no such row");
    }

    const LEASE: chrono::Duration = chrono::Duration::minutes(5);

    #[tokio::test]
    async fn a_work_lease_has_one_holder_until_it_is_released_or_lapses() {
        let db = in_memory_db().await;
        active_row(&db, "g-lease").await;
        db.merge_generation_metadata_if_active("g-lease", &serde_json::json!({ "keep": 1 })).await.unwrap();
        assert!(db.lease_generation_work("g-lease", "delivery", "a", LEASE).await.unwrap());
        assert!(!db.lease_generation_work("g-lease", "delivery", "b", LEASE).await.unwrap(), "a live lease holds the others off");
        assert!(db.lease_generation_work("g-lease", "delivery", "a", LEASE).await.unwrap(), "its holder may take it again");
        assert!(db.lease_generation_work("g-lease", "other", "b", LEASE).await.unwrap(), "other work is its own lease");

        db.release_generation_work("g-lease", "delivery", "b").await.unwrap();
        assert!(!db.lease_generation_work("g-lease", "delivery", "b", LEASE).await.unwrap(), "only the holder can release it");
        db.release_generation_work("g-lease", "delivery", "a").await.unwrap();
        assert!(db.lease_generation_work("g-lease", "delivery", "b", LEASE).await.unwrap(), "a released lease is free");

        // A holder that died: its lease is older than the term asked for.
        assert!(db.lease_generation_work("g-lease", "delivery", "c", chrono::Duration::zero()).await.unwrap(), "a lapsed lease is free");
        let m = metadata_of(&db, "g-lease").await;
        assert_eq!(m["leases"]["delivery"]["by"], "c", "{m}");
        assert!(m["leases"]["delivery"]["at"].is_string(), "{m}");
        assert_eq!(m["keep"], 1, "the rest of the metadata is untouched: {m}");
    }

    #[tokio::test]
    async fn racing_lease_claimants_produce_exactly_one_holder() {
        let db = std::sync::Arc::new(in_memory_db().await);
        active_row(&db, "g-lrace").await;
        let mut tasks = Vec::new();
        for i in 0..8 {
            let db = db.clone();
            tasks.push(tokio::spawn(async move { db.lease_generation_work("g-lrace", "delivery", &format!("o{i}"), LEASE).await.unwrap() }));
        }
        let mut wins = 0;
        for t in tasks {
            if t.await.unwrap() { wins += 1; }
        }
        assert_eq!(wins, 1);
    }

    /// A terminal row's work is done, and is never leased. A generation with
    /// no row yet is: its insert is spawned at submit, a fast poll can outrun
    /// it, and no poller works on a row it cannot list.
    #[tokio::test]
    async fn a_terminal_row_is_never_leased_and_a_missing_one_always_is() {
        let db = in_memory_db().await;
        active_row(&db, "g-lterm").await;
        db.update_generation_if_active("g-lterm", "completed", 100, None, None, Some(chrono::Utc::now()), None).await.unwrap();
        assert!(!db.lease_generation_work("g-lterm", "delivery", "a", LEASE).await.unwrap());
        assert!(db.lease_generation_work("g-none", "delivery", "a", LEASE).await.unwrap());
        assert!(db.get_generation("g-none").await.unwrap().is_none(), "and no row is made for it");
    }

    // ─── Idempotency-Key records ────────────────────────────────────────────

    use crate::db::{IdempotencyBegin, StoredResponse};

    fn stored(status: u16, body: &str) -> StoredResponse {
        StoredResponse {
            status,
            headers: vec![("content-type".into(), "application/json".into())],
            body: body.as_bytes().to_vec(),
        }
    }

    #[tokio::test]
    async fn a_finished_request_replays_and_a_new_body_conflicts() {
        let db = in_memory_db().await;
        let lease = crate::management::reservations::LEASE;
        assert!(matches!(db.idempotency_begin_held("org:a", "k1", "hash-1", "req-1", lease).await.unwrap(), IdempotencyBegin::Started));
        assert!(matches!(db.idempotency_begin_held("org:a", "k1", "hash-1", "req-2", lease).await.unwrap(), IdempotencyBegin::InProgress));
        db.idempotency_complete_held("org:a", "k1", "req-1", &stored(200, r#"{"id":"litegen-3d-x"}"#)).await.unwrap();
        match db.idempotency_begin_held("org:a", "k1", "hash-1", "req-3", lease).await.unwrap() {
            IdempotencyBegin::Replay(r) => {
                assert_eq!(r.status, 200);
                assert_eq!(r.body, br#"{"id":"litegen-3d-x"}"#.to_vec());
                assert_eq!(r.headers, vec![("content-type".to_string(), "application/json".to_string())]);
            }
            other => panic!("expected a replay, got {other:?}"),
        }
        assert!(matches!(db.idempotency_begin_held("org:a", "k1", "hash-2", "req-4", lease).await.unwrap(), IdempotencyBegin::Conflict));
        // Another owner's key of the same spelling is its own record.
        assert!(matches!(db.idempotency_begin_held("org:b", "k1", "hash-2", "req-5", lease).await.unwrap(), IdempotencyBegin::Started));
    }

    #[tokio::test]
    async fn a_released_key_can_run_again() {
        let db = in_memory_db().await;
        let lease = crate::management::reservations::LEASE;
        assert!(matches!(db.idempotency_begin_held("s", "k", "h", "req-1", lease).await.unwrap(), IdempotencyBegin::Started));
        db.idempotency_release_held("s", "k", "req-1").await.unwrap();
        assert!(matches!(db.idempotency_begin_held("s", "k", "h2", "req-2", lease).await.unwrap(), IdempotencyBegin::Started));
    }

    #[tokio::test]
    async fn a_record_older_than_a_day_is_forgotten_and_a_stale_run_abandoned() {
        let db = in_memory_db().await;
        let lease = crate::management::reservations::LEASE;
        db.idempotency_begin_held("s", "old", "h", "req-old", lease).await.unwrap();
        db.idempotency_complete_held("s", "old", "req-old", &stored(200, "{}")).await.unwrap();
        // With no lease, as an older replica writes it: the 15-minute rule.
        sqlx::query("INSERT INTO idempotency_keys (scope, idem_key, request_hash, state, created_at) VALUES ('s', 'stuck', 'h', 'in_progress', ?)")
            .bind(chrono::Utc::now())
            .execute(db.pool())
            .await
            .unwrap();
        let day_ago = chrono::Utc::now() - chrono::Duration::hours(25);
        let quarter_ago = chrono::Utc::now() - chrono::Duration::minutes(16);
        sqlx::query("UPDATE idempotency_keys SET created_at = ? WHERE idem_key = 'old'").bind(day_ago).execute(db.pool()).await.unwrap();
        sqlx::query("UPDATE idempotency_keys SET created_at = ? WHERE idem_key = 'stuck'").bind(quarter_ago).execute(db.pool()).await.unwrap();
        // Past 24 h the key is free again, whatever body comes with it.
        assert!(matches!(db.idempotency_begin_held("s", "old", "another", "req-1", lease).await.unwrap(), IdempotencyBegin::Started));
        // A run that never finished within 15 minutes died with its process.
        assert!(matches!(db.idempotency_begin_held("s", "stuck", "h", "req-2", lease).await.unwrap(), IdempotencyBegin::Started));
    }

    #[tokio::test]
    async fn racing_first_requests_start_exactly_once() {
        let db = std::sync::Arc::new(in_memory_db().await);
        let mut tasks = Vec::new();
        for i in 0..8 {
            let db = db.clone();
            tasks.push(tokio::spawn(async move {
                db.idempotency_begin_held("s", "race", "h", &format!("req-{i}"), crate::management::reservations::LEASE).await.unwrap()
            }));
        }
        let mut started = 0;
        for t in tasks {
            if matches!(t.await.unwrap(), IdempotencyBegin::Started) { started += 1; }
        }
        assert_eq!(started, 1);
    }

    // ─── leases and idempotency records on the database's clock (R8) ─────
    // `with_clock_offset` sets "the database's clock" apart from this
    // process's, which is how a replica whose own clock is off looks.

    /// A replica whose clock runs 6 min ahead of the database's must not take
    /// over a delivery lease another replica took just now (stamped on the
    /// database's clock): on the database's clock it is fresh.
    #[tokio::test]
    async fn a_lease_another_replica_took_is_judged_on_the_databases_clock() {
        let db = in_memory_db().await.with_clock_offset(-chrono::Duration::minutes(6));
        active_row(&db, "g-skew").await;
        let just_now_on_the_database = (chrono::Utc::now() - chrono::Duration::minutes(6)).to_rfc3339();
        db.merge_generation_metadata_if_active("g-skew", &serde_json::json!({
            "leases": { "delivery": { "by": "elsewhere", "at": just_now_on_the_database } },
        })).await.unwrap();
        assert!(
            !db.lease_generation_work("g-skew", "delivery", "here", crate::proxy::delivery_lease::DELIVERY_LEASE).await.unwrap(),
            "a lease taken just now on the database's clock is not 6 min old"
        );
    }

    /// A lease is stamped on the database's clock, so every replica judges it
    /// on the same one.
    #[tokio::test]
    async fn a_lease_is_stamped_on_the_databases_clock() {
        let db = in_memory_db().await.with_clock_offset(chrono::Duration::minutes(6));
        active_row(&db, "g-stamp").await;
        assert!(db.lease_generation_work("g-stamp", "delivery", "here", crate::proxy::delivery_lease::DELIVERY_LEASE).await.unwrap());
        let m = metadata_of(&db, "g-stamp").await;
        let at = chrono::DateTime::parse_from_rfc3339(m["leases"]["delivery"]["at"].as_str().unwrap()).unwrap();
        let off = (at.with_timezone(&chrono::Utc) - (chrono::Utc::now() + chrono::Duration::minutes(6))).num_seconds().abs();
        assert!(off < 5, "the lease was stamped {off} s from the database's now: {m}");
    }

    /// A replica whose clock runs 16 min ahead must not judge another
    /// replica's in-flight `Idempotency-Key` stale (15 min) and run the
    /// request again: on the database's clock the record is fresh.
    #[tokio::test]
    async fn an_in_progress_key_is_judged_on_the_databases_clock() {
        use crate::db::IdempotencyBegin;
        let db = in_memory_db().await.with_clock_offset(-chrono::Duration::minutes(16));
        let just_now_on_the_database = chrono::Utc::now() - chrono::Duration::minutes(16);
        sqlx::query("INSERT INTO idempotency_keys (scope, idem_key, request_hash, state, created_at) VALUES ('s', 'k', 'h', 'in_progress', ?)")
            .bind(just_now_on_the_database)
            .execute(db.pool())
            .await
            .unwrap();
        let begun = db.idempotency_begin_held("s", "k", "h", "req-1", crate::management::reservations::LEASE).await.unwrap();
        assert_eq!(begun, IdempotencyBegin::InProgress, "the other replica's run is in progress, not stale");
    }
    #[tokio::test]
    async fn generations_due_for_retention_respect_app_cutoffs() {
        use crate::types::AppFileAccessUpsert;
        let db = in_memory_db().await;
        let org = make_org(&Uuid::new_v4().to_string(), "keeps");
        db.create_organization(&org).await.unwrap();
        let (a, b) = (make_app(&Uuid::new_v4().to_string(), &org.id, "a"), make_app(&Uuid::new_v4().to_string(), &org.id, "b"));
        db.create_application(&a).await.unwrap();
        db.create_application(&b).await.unwrap();
        for (id, app) in [("g-a", Some(a.id.as_str())), ("g-b", Some(b.id.as_str())), ("g-none", None), ("g-running", Some(a.id.as_str()))] {
            db.insert_generation(id, None, "mock/mesh-3d", "mock", "model3d", None, 0.0, Some(&org.id), app).await.unwrap();
            if id != "g-running" {
                db.update_generation_if_active(id, "completed", 100, None, None, Some(chrono::Utc::now()), None).await.unwrap();
            }
        }
        db.put_app_file_access(&AppFileAccessUpsert {
            app_id: a.id.clone(), mode: None, owner_scope: None, signed_url_ttl_seconds: None,
            keep_outputs: None, retention_days: Some(1), signed_links: None,
        }).await.unwrap();
        assert_eq!(db.apps_with_retention().await.unwrap(), vec![(a.id.clone(), 1)]);
        let later = chrono::Utc::now() + chrono::Duration::days(2);
        let ids = |rows: Vec<crate::types::Generation>| rows.into_iter().map(|g| g.id).collect::<Vec<_>>();
        assert_eq!(ids(db.generations_due_for_retention(Some(&a.id), later, 10).await.unwrap()), ["g-a"], "finished ones only");
        let mut rest = ids(db.generations_due_for_retention(None, later, 10).await.unwrap());
        rest.sort();
        assert_eq!(rest, ["g-b", "g-none"], "every app without its own retention, and none");
        assert!(db.generations_due_for_retention(Some(&a.id), chrono::Utc::now() - chrono::Duration::days(1), 10).await.unwrap().is_empty(), "none older than that");
        assert_eq!(db.generations_due_for_retention(None, later, 1).await.unwrap().len(), 1, "the limit");
        db.mark_files_deleted("g-a", chrono::Utc::now(), "retention").await.unwrap();
        assert!(db.generations_due_for_retention(Some(&a.id), later, 10).await.unwrap().is_empty(), "swept once");
        let g = db.get_generation("g-a").await.unwrap().unwrap();
        assert!(g.files_deleted_at.is_some());
        assert_eq!(g.files_deleted_reason.as_deref(), Some("retention"));
    }

    /// A row whose files the sweep failed to delete goes behind the rows not
    /// yet tried (P12), so rows that keep failing can't fill every batch.
    #[tokio::test]
    async fn a_row_whose_delete_failed_goes_behind_the_others() {
        let db = in_memory_db().await;
        let org = make_org(&Uuid::new_v4().to_string(), "fails");
        db.create_organization(&org).await.unwrap();
        let app = make_app(&Uuid::new_v4().to_string(), &org.id, "a");
        db.create_application(&app).await.unwrap();
        for (id, made) in [("g-old", "-3 days"), ("g-new", "-2 days")] {
            db.insert_generation(id, None, "mock/mesh-3d", "mock", "model3d", None, 0.0, Some(&org.id), Some(&app.id)).await.unwrap();
            db.update_generation_if_active(id, "completed", 100, None, None, Some(chrono::Utc::now()), None).await.unwrap();
            sqlx::query("UPDATE generations SET created_at = datetime('now', ?) WHERE id = ?").bind(made).bind(id).execute(db.pool()).await.unwrap();
        }
        let ids = |rows: Vec<crate::types::Generation>| rows.into_iter().map(|g| g.id).collect::<Vec<_>>();
        let later = chrono::Utc::now();
        assert_eq!(ids(db.generations_due_for_retention(Some(&app.id), later, 1).await.unwrap()), ["g-old"], "oldest first");

        db.note_files_delete_failed("g-old").await.unwrap();
        assert_eq!(ids(db.generations_due_for_retention(Some(&app.id), later, 1).await.unwrap()), ["g-new"], "the failed row goes behind");
        assert_eq!(ids(db.generations_due_for_retention(Some(&app.id), later, 10).await.unwrap()), ["g-new", "g-old"], "and is still due");
    }

    #[tokio::test]
    async fn link_revocations_take_the_latest_scope() {
        let db = in_memory_db().await;
        let t = |secs: i64| chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0).unwrap();
        assert_eq!(db.links_revoked_at(Some("a"), "g").await.unwrap(), None);
        db.revoke_links("app", "a", t(1_000)).await.unwrap();
        assert_eq!(db.links_revoked_at(Some("a"), "g").await.unwrap(), Some(t(1_000)));
        assert_eq!(db.links_revoked_at(Some("b"), "g").await.unwrap(), None, "another app's");
        db.revoke_links("generation", "g", t(2_000)).await.unwrap();
        assert_eq!(db.links_revoked_at(Some("a"), "g").await.unwrap(), Some(t(2_000)), "the latest of app and generation");
        assert_eq!(db.links_revoked_at(Some("a"), "h").await.unwrap(), Some(t(1_000)));
        db.revoke_links("gateway", "", t(1_500)).await.unwrap();
        assert_eq!(db.links_revoked_at(Some("b"), "h").await.unwrap(), Some(t(1_500)), "the gateway's reaches every app");
        assert_eq!(db.links_revoked_at(None, "g").await.unwrap(), Some(t(2_000)));
        db.revoke_links("app", "a", t(3_000)).await.unwrap();
        assert_eq!(db.links_revoked_at(Some("a"), "g").await.unwrap(), Some(t(3_000)), "a later revocation replaces the app's");
        db.revoke_links("app", "a", t(500)).await.unwrap();
        assert_eq!(db.links_revoked_at(Some("a"), "h").await.unwrap(), Some(t(3_000)), "an earlier time never un-revokes");
    }

    #[tokio::test]
    async fn app_file_access_round_trips_and_goes_with_its_app() {
        use crate::types::AppFileAccessUpsert;
        let db = in_memory_db().await;
        let org = make_org(&Uuid::new_v4().to_string(), "files");
        db.create_organization(&org).await.unwrap();
        let app = make_app(&Uuid::new_v4().to_string(), &org.id, "web");
        db.create_application(&app).await.unwrap();

        assert!(db.get_app_file_access(&app.id).await.unwrap().is_none());
        let put = |mode: Option<&str>, scope: Option<&str>, ttl: Option<i64>| AppFileAccessUpsert {
            app_id: app.id.clone(),
            mode: mode.map(str::to_string),
            owner_scope: scope.map(str::to_string),
            signed_url_ttl_seconds: ttl,
            keep_outputs: None,
            retention_days: None,
            signed_links: None,
        };
        db.put_app_file_access(&put(Some("signed"), None, Some(900))).await.unwrap();
        let row = db.get_app_file_access(&app.id).await.unwrap().expect("stored");
        assert_eq!((row.mode.as_deref(), row.owner_scope.as_deref(), row.signed_url_ttl_seconds), (Some("signed"), None, Some(900)));
        db.put_app_file_access(&put(None, Some("org"), None)).await.unwrap();
        let row = db.get_app_file_access(&app.id).await.unwrap().expect("stored");
        assert_eq!((row.mode.as_deref(), row.owner_scope.as_deref(), row.signed_url_ttl_seconds), (None, Some("org"), None), "a put replaces every field");
        // Keeping, retention and links round-trip, and are replaced with the rest.
        db.put_app_file_access(&AppFileAccessUpsert {
            keep_outputs: Some(false),
            retention_days: Some(30),
            signed_links: Some("litegen".into()),
            ..put(None, None, None)
        })
        .await
        .unwrap();
        let row = db.get_app_file_access(&app.id).await.unwrap().expect("stored");
        assert_eq!((row.keep_outputs, row.retention_days, row.signed_links.as_deref()), (Some(false), Some(30), Some("litegen")));
        db.put_app_file_access(&put(None, Some("org"), None)).await.unwrap();
        let row = db.get_app_file_access(&app.id).await.unwrap().expect("stored");
        assert_eq!((row.keep_outputs, row.retention_days, row.signed_links.as_deref()), (None, None, None));
        assert!(db.delete_app_file_access(&app.id).await.unwrap());
        assert!(!db.delete_app_file_access(&app.id).await.unwrap());

        // Deleting the app, or its org, takes the row with it.
        db.put_app_file_access(&put(Some("private"), None, None)).await.unwrap();
        assert!(db.delete_application(&app.id).await.unwrap());
        assert!(db.get_app_file_access(&app.id).await.unwrap().is_none());
        let app2 = make_app(&Uuid::new_v4().to_string(), &org.id, "web2");
        db.create_application(&app2).await.unwrap();
        db.put_app_file_access(&AppFileAccessUpsert { app_id: app2.id.clone(), ..put(Some("private"), None, None) }).await.unwrap();
        assert!(db.delete_organization(&org.id).await.unwrap());
        assert!(db.get_app_file_access(&app2.id).await.unwrap().is_none());
    }

    // ── S2: a database file litegen makes, or can't use ──

    /// A path's text, as a message about it names it.
    fn shown(path: &std::path::Path) -> String {
        path.display().to_string()
    }

    #[tokio::test]
    async fn a_url_that_asks_to_create_the_database_creates_its_folder_too() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("nested/data/litegen.db");
        let db = SqliteDatabase::connect(&format!("sqlite://{}?mode=rwc", shown(&file))).await.expect("opens");
        db.create_api_key("k", "h", "lg-k", None, None, "read", None).await.unwrap();
        assert!(file.is_file(), "the database is {}", shown(&file));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_database_litegen_cannot_open_is_named_with_its_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        // The file can't be made in a directory litegen can't write, nor a directory below it.
        let in_it = SqliteDatabase::connect(&format!("sqlite://{}/litegen.db?mode=rwc", shown(&locked))).await.err();
        let below_it = SqliteDatabase::connect(&format!("sqlite://{}/sub/litegen.db?mode=rwc", shown(&locked))).await.err();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        for (file, err) in [(locked.join("litegen.db"), in_it), (locked.join("sub/litegen.db"), below_it)] {
            let msg = err.unwrap_or_else(|| panic!("{} opened in a directory litegen can't write", shown(&file))).to_string();
            assert!(msg.contains(&shown(&file)) && msg.contains("writable"), "names the file and the fix: {msg}");
            assert!(!msg.contains("add ?mode=rwc"), "the URL has mode=rwc already: {msg}");
        }
    }

    #[tokio::test]
    async fn a_missing_database_without_mode_rwc_is_still_refused() {
        let dir = tempfile::tempdir().unwrap();
        let absent = dir.path().join("absent");
        let file = absent.join("litegen.db");
        let err = SqliteDatabase::connect(&format!("sqlite://{}", shown(&file))).await.err();
        let msg = err.unwrap_or_else(|| panic!("no mode=rwc, yet {} was opened", shown(&file))).to_string();
        assert!(!absent.exists(), "no directory is made for a URL that doesn't ask to create its database");
        assert!(msg.contains(&shown(&file)) && msg.contains("add ?mode=rwc"), "names the file and the fix: {msg}");
    }

    /// SQLite opens a file it can't write read-only, so the failure comes at
    /// the first migration: SQLITE_READONLY (8), or, for a directory where its
    /// journal can't be made, SQLITE_READONLY_DIRECTORY (1544).
    #[cfg(unix)]
    #[tokio::test]
    async fn a_database_litegen_cannot_write_is_named_with_its_path() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &std::path::Path, mode: u32| std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let read_only_file = dir.path().join("litegen.db");
        std::fs::write(&read_only_file, "").unwrap();
        mode(&read_only_file, 0o444);
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        let in_read_only_dir = locked.join("litegen.db");
        std::fs::write(&in_read_only_dir, "").unwrap();
        mode(&in_read_only_dir, 0o666);
        mode(&locked, 0o555);
        let file_err = SqliteDatabase::connect(&format!("sqlite://{}", shown(&read_only_file))).await.err();
        let dir_err = SqliteDatabase::connect(&format!("sqlite://{}", shown(&in_read_only_dir))).await.err();
        mode(&locked, 0o755);
        for (file, err) in [(read_only_file, file_err), (in_read_only_dir, dir_err)] {
            let msg = err.unwrap_or_else(|| panic!("{} was migrated, but litegen can't write it", shown(&file))).to_string();
            assert!(msg.contains(&shown(&file)) && msg.contains("writable"), "names the file and the fix: {msg}");
        }
    }

    // ── S2 fix-up M1: a database setting that isn't a SQLite URL ──

    /// The error `connect(value)` refuses `value` with, if it repeats none of
    /// the credentials or the host in it; otherwise what went wrong.
    async fn refusal_of(value: &str) -> Result<String, String> {
        let msg = match SqliteDatabase::connect(value).await {
            Ok(_) => return Err(format!("{value}: opened as SQLite")),
            Err(e) => e.to_string(),
        };
        if msg.contains("s3cret-pw") || msg.contains("db.example") {
            return Err(format!("{value}: repeated in {msg}"));
        }
        Ok(msg)
    }

    /// sqlx reads all of a URL but a `sqlite:` prefix as a file name, so a URL
    /// meant for another database was a path: named in errors, password and
    /// all, and, with `?mode=rwc`, made into folders and a new SQLite database.
    /// It is refused, naming only its scheme, and only when that is a scheme.
    #[tokio::test]
    async fn a_url_for_another_database_is_refused_naming_only_its_scheme() {
        let mut wrong = Vec::new();
        for (url, scheme) in [
            ("mysql://litegen:s3cret-pw@db.example:3306/litegen", "mysql"),
            ("postgresql+psycopg://litegen:s3cret-pw@db.example:5432/litegen", "postgresql+psycopg"),
            ("POSTGRES://litegen:s3cret-pw@db.example:5432/litegen", "POSTGRES"),
        ] {
            match refusal_of(url).await {
                Ok(msg) if msg.contains(&format!("`{scheme}`")) => {}
                Ok(msg) => wrong.push(format!("{url}: doesn't name `{scheme}`: {msg}")),
                Err(problem) => wrong.push(problem),
            }
        }
        // What comes before `://` here is no scheme RFC 3986 allows (a quote
        // mark; 33 characters), so it may be anything: none of it is named.
        for (url, before) in [
            ("\"postgres://litegen:s3cret-pw@db.example:5432/litegen\"", "\"postgres"),
            ("abcdefghijklmnopqrstuvwxyz1234567://litegen:s3cret-pw@db.example/litegen", "abcdefghijklmnopqrstuvwxyz1234567"),
        ] {
            match refusal_of(url).await {
                Ok(msg) if !msg.contains(before) && msg.contains("this database URL's scheme") => {}
                Ok(msg) => wrong.push(format!("{url}: names its text before :// or doesn't say why: {msg}")),
                Err(problem) => wrong.push(problem),
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    /// A value with neither `sqlite:` nor a scheme is read as a SQLite file
    /// name, as before S2. It may be no path at all, though: a Postgres
    /// keyword/value string with its password, or a `postgres:/` typo. Its
    /// error repeats none of it, and offers no `?mode=rwc` for it.
    #[tokio::test]
    async fn a_value_with_no_scheme_is_never_repeated_in_an_error() {
        let mut wrong = Vec::new();
        for value in ["host=db.example user=litegen password=s3cret-pw dbname=litegen", "postgres:/litegen:s3cret-pw@db.example:5432/litegen"] {
            match refusal_of(value).await {
                Ok(msg) if !msg.contains("?mode=rwc") && msg.contains("sqlite://") => {}
                Ok(msg) => wrong.push(format!("{value}: offers ?mode=rwc, or doesn't say how to write a SQLite URL: {msg}")),
                Err(problem) => wrong.push(problem),
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    /// Nor is a directory made for such a value with `?mode=rwc`: that
    /// `postgres:/` typo would become folders named after its password. A path
    /// in a temporary directory stands in for it, since before the fix the
    /// typo itself would make its folders in the working directory.
    #[tokio::test]
    async fn a_value_with_no_scheme_gets_no_directory() {
        let dir = tempfile::tempdir().unwrap();
        let absent = dir.path().join("absent");
        let err = SqliteDatabase::connect(&format!("{}?mode=rwc", shown(&absent.join("litegen.db")))).await.err();
        assert!(!absent.exists(), "a directory was made for a value that isn't a SQLite URL");
        let msg = err.expect("opened in a directory that doesn't exist").to_string();
        assert!(!msg.contains(&shown(dir.path())), "the value is repeated: {msg}");
    }

    /// Guard: the SQLite forms besides `sqlite://` still open: a bare path,
    /// and `sqlite:` without the slashes.
    #[tokio::test]
    async fn a_bare_path_and_the_short_sqlite_form_still_open() {
        let dir = tempfile::tempdir().unwrap();
        let (bare, short) = (dir.path().join("bare.db"), dir.path().join("short.db"));
        for url in [format!("{}?mode=rwc", shown(&bare)), format!("sqlite:{}?mode=rwc", shown(&short))] {
            let db = SqliteDatabase::connect(&url).await.unwrap_or_else(|e| panic!("{url}: {e}"));
            db.create_api_key("k", "h", "lg-k", None, None, "read", None).await.unwrap();
        }
        assert!(bare.is_file() && short.is_file(), "the databases are {} and {}", shown(&bare), shown(&short));
    }

    // ─── the webhook outbox (migration 029): which terminal writes record a delivery ───

    /// A key called `name`, with `webhook` as its webhook_url: its id.
    async fn webhook_key(db: &SqliteDatabase, name: &str, webhook: Option<&str>) -> Uuid {
        db.create_api_key(name, &format!("hash-{name}"), &format!("lg-{name}"), None, None, "generate,read", webhook).await.unwrap().id
    }

    /// Generation `id` of `key`: `media`, with the vendor job id `job`, in the
    /// default org and app.
    async fn job_of(db: &SqliteDatabase, id: &str, key: Option<&Uuid>, media: &str, job: Option<&str>) {
        use crate::api::middleware::{DEFAULT_APP_ID, DEFAULT_ORG_ID};
        db.insert_generation(id, key, "mock/x", "mock", media, job, 0.0, Some(DEFAULT_ORG_ID), Some(DEFAULT_APP_ID)).await.unwrap();
    }

    /// The terminal writes that send a terminal webhook record its delivery,
    /// pending, in the same statement: a completion and a failure (the
    /// poller's, a client poll's or a reap's guarded write), a cancel, a late
    /// image, and an image row with no vendor job id that fails (a reap). It
    /// is due after its first-attempt grace, 30 s (migration 032; review
    /// 2026-11-15 round 2, finding 2): until then it is the terminal path's
    /// to send, after it has settled the job.
    #[tokio::test]
    async fn a_terminal_write_records_a_pending_delivery_for_a_key_with_a_webhook() {
        let db = in_memory_db().await;
        let key = webhook_key(&db, "hooked", Some("https://hooks.example/h")).await;
        let now = chrono::Utc::now;
        job_of(&db, "g-done", Some(&key), "video", Some("j1")).await;
        db.update_generation_if_active("g-done", "completed", 100, Some("https://cdn.example/v.mp4"), None, Some(now()), None).await.unwrap();
        job_of(&db, "g-failed", Some(&key), "model3d", Some("j2")).await;
        db.update_generation_if_active("g-failed", "processing", 50, None, None, None, None).await.unwrap();
        db.update_generation_if_active("g-failed", "failed", 50, None, Some("vendor failed"), Some(now()), Some(&serde_json::json!({ "x": 1 }))).await.unwrap();
        job_of(&db, "g-cancelled", Some(&key), "video", Some("j3")).await;
        db.cancel_generation("g-cancelled").await.unwrap().expect("cancelled");
        job_of(&db, "g-late-image", Some(&key), "image", Some("j4")).await;
        db.update_generation_if_active("g-late-image", "completed", 100, Some("https://cdn.example/i.png"), None, Some(now()), None).await.unwrap();
        job_of(&db, "g-image-reaped", Some(&key), "image", None).await;
        db.update_generation_if_active("g-image-reaped", "failed", 0, None, Some("timed out"), Some(now()), None).await.unwrap();

        for id in ["g-done", "g-failed", "g-cancelled", "g-late-image", "g-image-reaped"] {
            let d = db.webhook_outbox_of(id).await.unwrap().unwrap_or_else(|| panic!("{id}: no delivery recorded"));
            assert_eq!((d.status.as_str(), d.attempts, d.key_id.as_str()), ("pending", 0, key.to_string().as_str()), "{id}: {d:?}");
            let uuid = d.id.strip_prefix("whd-").unwrap_or_else(|| panic!("{id}: {}", d.id));
            assert!(Uuid::parse_str(uuid).is_ok(), "{id}: the delivery id is whd- and a UUID: {}", d.id);
            let grace = chrono::Duration::from_std(crate::proxy::webhook::OUTBOX_FIRST_ATTEMPT_GRACE).unwrap();
            let wait = d.next_attempt_at - now();
            assert!(wait > grace - chrono::Duration::seconds(5) && wait <= grace, "{id}: due after the grace ({grace:?}), in {wait:?}: {d:?}");
            assert!(d.claimed_by.is_none() && d.finished_at.is_none(), "{id}: {d:?}");
            assert_eq!(d.org_id.as_deref(), Some(crate::api::middleware::DEFAULT_ORG_ID), "{id}");
            assert_eq!(d.app_id.as_deref(), Some(crate::api::middleware::DEFAULT_APP_ID), "{id}");
        }
        let id_of = |id: &'static str| {
            let db = &db;
            async move { db.webhook_outbox_of(id).await.unwrap().unwrap().id }
        };
        assert_ne!(id_of("g-done").await, id_of("g-failed").await, "each delivery has an id of its own");
    }

    /// No delivery where no terminal webhook is sent: a key with no
    /// webhook_url, a key whose webhook_url was cleared to `""` (as a PATCH
    /// stored it before migration 032; review 2026-11-15 round 2, finding 4),
    /// a job with no key, a sync image result (an image with no vendor job
    /// id, completed by the request that made it), a write that isn't
    /// terminal, and a second terminal write (one delivery per generation).
    /// Written before the trigger, as a guard: it fails if the trigger fires
    /// on a write that sends no webhook today.
    #[tokio::test]
    async fn no_delivery_is_recorded_where_no_terminal_webhook_is_sent() {
        let db = in_memory_db().await;
        let plain = webhook_key(&db, "plain", None).await;
        let hooked = webhook_key(&db, "hooked", Some("https://hooks.example/h")).await;
        let cleared = webhook_key(&db, "cleared", Some("https://hooks.example/h")).await;
        sqlx::query("UPDATE api_keys SET webhook_url = '' WHERE id = ?").bind(cleared.to_string()).execute(db.pool()).await.unwrap();
        let now = || Some(chrono::Utc::now());
        job_of(&db, "g-plain", Some(&plain), "video", Some("j1")).await;
        db.update_generation_if_active("g-plain", "completed", 100, None, None, now(), None).await.unwrap();
        job_of(&db, "g-cleared", Some(&cleared), "video", Some("j5")).await;
        db.update_generation_if_active("g-cleared", "completed", 100, None, None, now(), None).await.unwrap();
        job_of(&db, "g-keyless", None, "video", Some("j2")).await;
        db.update_generation_if_active("g-keyless", "failed", 0, None, Some("x"), now(), None).await.unwrap();
        job_of(&db, "g-sync-image", Some(&hooked), "image", None).await;
        db.update_generation_if_active("g-sync-image", "completed", 100, Some("https://cdn.example/i.png"), None, now(), Some(&serde_json::json!({ "assets": [] }))).await.unwrap();
        job_of(&db, "g-running", Some(&hooked), "video", Some("j3")).await;
        db.update_generation_if_active("g-running", "processing", 40, None, None, None, None).await.unwrap();
        for id in ["g-plain", "g-cleared", "g-keyless", "g-sync-image", "g-running"] {
            assert_eq!(db.webhook_outbox_of(id).await.unwrap(), None, "{id}");
        }

        job_of(&db, "g-twice", Some(&hooked), "video", Some("j4")).await;
        db.update_generation_if_active("g-twice", "completed", 100, None, None, now(), None).await.unwrap();
        let first = db.webhook_outbox_of("g-twice").await.unwrap();
        db.update_generation_status("g-twice", "failed", 100, None, Some("again"), now()).await.unwrap();
        assert_eq!(db.webhook_outbox_of("g-twice").await.unwrap(), first, "a row already terminal records no second delivery");
    }

    /// One org's backlog can't take every claim: a claimant holds at most
    /// `per_org` of one org's deliveries at once, those it holds already
    /// counted, so another org's due delivery is claimed beside them, and the
    /// rest of the first org's wait for another claimant or a later claim.
    #[tokio::test]
    async fn a_claimant_holds_at_most_its_share_of_one_orgs_deliveries() {
        let db = in_memory_db().await;
        let key = webhook_key(&db, "hooked", Some("https://hooks.example/h")).await;
        for (id, org) in [("a1", "org-a"), ("a2", "org-a"), ("a3", "org-a"), ("b1", "org-b")] {
            db.insert_generation(id, Some(&key), "mock/x", "mock", "video", Some("j"), 0.0, Some(org), None).await.unwrap();
            db.update_generation_if_active(id, "completed", 100, None, None, Some(chrono::Utc::now()), None).await.unwrap();
        }
        // Their first-attempt grace is over.
        sqlx::query("UPDATE webhook_outbox SET next_attempt_at = strftime('%Y-%m-%d %H:%M:%f', 'now', '-1 seconds')").execute(db.pool()).await.unwrap();
        let hold = chrono::Duration::seconds(60);
        let orgs = |claimed: Vec<crate::db::OutboxDelivery>| {
            let mut orgs: Vec<String> = claimed.into_iter().map(|d| d.org_id.unwrap_or_default()).collect();
            orgs.sort();
            orgs
        };
        assert_eq!(orgs(db.claim_due_webhook_deliveries("r1", 10, 2, hold).await.unwrap()), ["org-a", "org-a", "org-b"]);
        assert!(db.claim_due_webhook_deliveries("r1", 10, 2, hold).await.unwrap().is_empty(), "r1 holds its share of org-a already");
        assert_eq!(orgs(db.claim_due_webhook_deliveries("r2", 10, 2, hold).await.unwrap()), ["org-a"], "another claimant takes the third");
    }

    /// Deleting an app, or an org, takes its outbox deliveries along with
    /// its generations and keys.
    #[tokio::test]
    async fn deleting_an_app_or_an_org_takes_its_outbox_deliveries() {
        let db = in_memory_db().await;
        for (tag, by_app) in [("app", true), ("org", false)] {
            let org = make_org(&Uuid::new_v4().to_string(), &format!("{tag}-org"));
            db.create_organization(&org).await.unwrap();
            let app = make_app(&Uuid::new_v4().to_string(), &org.id, &format!("{tag}-app"));
            db.create_application(&app).await.unwrap();
            let key = db
                .create_api_key_scoped(&org.id, &app.id, &format!("pk_{tag}"), "k", &format!("hash-{tag}"), "lg-k", None, None, "generate", Some("https://hooks.example/h"), None)
                .await
                .unwrap();
            let id = format!("g-{tag}");
            db.insert_generation(&id, Some(&key.id), "mock/v", "mock", "video", Some("j"), 0.0, Some(&org.id), Some(&app.id)).await.unwrap();
            db.update_generation_if_active(&id, "completed", 100, None, None, Some(chrono::Utc::now()), None).await.unwrap();
            assert!(db.webhook_outbox_of(&id).await.unwrap().is_some(), "{tag}: recorded");

            if by_app {
                assert!(db.delete_application(&app.id).await.unwrap());
            } else {
                assert!(db.delete_organization(&org.id).await.unwrap());
            }
            assert_eq!(db.webhook_outbox_of(&id).await.unwrap(), None, "deleting the {tag} took its delivery");
        }
    }

    /// An attempt sent from memory (its job recorded no outbox delivery)
    /// carries its job's org and app, as an outbox attempt does, so deleting
    /// the app or the org takes it too. insert_webhook_delivery set neither
    /// from migration 008 on.
    #[tokio::test]
    async fn an_attempt_sent_from_memory_carries_its_jobs_org_and_app() {
        let db = in_memory_db().await;
        for (tag, by_app) in [("app", true), ("org", false)] {
            let org = make_org(&Uuid::new_v4().to_string(), &format!("mem-{tag}-org"));
            db.create_organization(&org).await.unwrap();
            let app = make_app(&Uuid::new_v4().to_string(), &org.id, &format!("mem-{tag}-app"));
            db.create_application(&app).await.unwrap();
            let key = db
                .create_api_key_scoped(&org.id, &app.id, &format!("pk_mem_{tag}"), "k", &format!("hash-mem-{tag}"), "lg-k", None, None, "generate", None, None)
                .await
                .unwrap();
            let id = format!("g-mem-{tag}");
            db.insert_generation(&id, Some(&key.id), "mock/v", "mock", "video", Some("j"), 0.0, Some(&org.id), Some(&app.id)).await.unwrap();
            let attempt = crate::types::WebhookDelivery {
                id: format!("wh-mem-{tag}"),
                key_id: key.id.to_string(),
                generation_id: id.clone(),
                url: "https://hooks.example/h".to_string(),
                attempt_number: 1,
                status_code: Some(200),
                success: true,
                response_body: None,
                error_message: None,
                payload_json: "{}".to_string(),
                created_at: chrono::Utc::now(),
                // A delivery sent from memory has an id of its own, in no outbox row.
                delivery_id: Some(format!("whd-{}", Uuid::new_v4())),
            };
            db.insert_webhook_delivery(&attempt).await.unwrap();
            let tenant: (Option<String>, Option<String>) =
                sqlx::query_as("SELECT org_id, app_id FROM webhook_deliveries WHERE id = ?").bind(&attempt.id).fetch_one(db.pool()).await.unwrap();
            assert_eq!(tenant, (Some(org.id.clone()), Some(app.id.clone())), "{tag}: the attempt carries its job's org and app");

            if by_app {
                assert!(db.delete_application(&app.id).await.unwrap());
            } else {
                assert!(db.delete_organization(&org.id).await.unwrap());
            }
            let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM webhook_deliveries WHERE id = ?").bind(&attempt.id).fetch_one(db.pool()).await.unwrap();
            assert_eq!(left, 0, "deleting the {tag} took the attempt");
        }
    }

    /// The retention sweep's purge of the webhook log: the deliveries that
    /// ended, and the attempts made, before the window go, a batch of at most
    /// `limit` each per call; a pending delivery stays however old it is, and
    /// so does everything within the window. On the database's clock.
    #[tokio::test]
    async fn the_webhook_log_purge_takes_what_ended_before_its_window_and_never_a_pending_delivery() {
        let db = in_memory_db().await;
        let key = webhook_key(&db, "hooked", Some("https://hooks.example/h")).await;
        // (generation, status, finished this many days ago)
        let deliveries = [("g-old-delivered", "delivered", Some(31)), ("g-old-failed", "failed", Some(31)), ("g-old-pending", "pending", None), ("g-recent", "delivered", Some(29))];
        for (id, status, ended) in deliveries {
            job_of(&db, id, Some(&key), "video", Some("j")).await;
            db.update_generation_if_active(id, "completed", 100, None, None, Some(chrono::Utc::now()), None).await.unwrap();
            let finished = ended.map(|days: i64| format!("-{days} days"));
            sqlx::query(
                "UPDATE webhook_outbox SET status = ?, created_at = strftime('%Y-%m-%d %H:%M:%f', 'now', '-400 days'), \
                 next_attempt_at = strftime('%Y-%m-%d %H:%M:%f', 'now', '-399 days'), \
                 finished_at = CASE WHEN ? IS NULL THEN NULL ELSE strftime('%Y-%m-%d %H:%M:%f', 'now', ?) END WHERE generation_id = ?",
            )
            .bind(status)
            .bind(&finished)
            .bind(&finished)
            .bind(id)
            .execute(db.pool())
            .await
            .unwrap();
        }
        // Attempts made 31, 31 and 29 days ago.
        for (n, days) in [(1, 31), (2, 31), (3, 29)] {
            let attempt = crate::types::WebhookDelivery {
                id: format!("wh-{n}"),
                key_id: key.to_string(),
                generation_id: "g-old-delivered".to_string(),
                url: "https://hooks.example/h".to_string(),
                attempt_number: n,
                status_code: Some(503),
                success: false,
                response_body: None,
                error_message: None,
                payload_json: String::new(),
                created_at: chrono::Utc::now(),
                delivery_id: None,
            };
            db.insert_webhook_delivery(&attempt).await.unwrap();
            sqlx::query("UPDATE webhook_deliveries SET created_at = datetime('now', ?) WHERE id = ?").bind(format!("-{days} days")).bind(&attempt.id).execute(db.pool()).await.unwrap();
        }

        let mut purged = Vec::new();
        for _ in 0..3 {
            purged.push(db.purge_webhook_log(30, 1).await.unwrap());
        }
        let batch = |deliveries, attempts| crate::db::WebhookLogPurged { deliveries, attempts };
        assert_eq!(purged, [batch(1, 1), batch(1, 1), batch(0, 0)], "one of each per batch, until none is left");
        for (id, status, _) in deliveries {
            let kept = db.webhook_outbox_of(id).await.unwrap().is_some();
            assert_eq!(kept, status == "pending" || id == "g-recent", "{id}");
        }
        let attempts: Vec<String> = sqlx::query_scalar("SELECT id FROM webhook_deliveries").fetch_all(db.pool()).await.unwrap();
        assert_eq!(attempts, ["wh-3"], "the attempt within the window stays");
    }

    /// A delivery that ended can be sent again (`POST .../redeliver`): it is
    /// due now, a fresh series under the same id (attempts back to 0), its
    /// last outcome cleared and when it was redelivered noted. A pending
    /// one, one redelivered within the cooldown, another key's and an unknown
    /// one change nothing.
    #[tokio::test]
    async fn a_finished_delivery_is_made_due_again_once_per_cooldown() {
        use crate::db::Redelivery;
        let db = in_memory_db().await;
        let key = webhook_key(&db, "hooked", Some("https://hooks.example/h")).await.to_string();
        let other = webhook_key(&db, "other", Some("https://hooks.example/o")).await.to_string();
        let mut ids = std::collections::HashMap::new();
        for (id, status) in [("g-delivered", "delivered"), ("g-failed", "failed"), ("g-pending", "pending")] {
            job_of(&db, id, Some(&Uuid::parse_str(&key).unwrap()), "video", Some("j")).await;
            db.update_generation_if_active(id, "completed", 100, None, None, Some(chrono::Utc::now()), None).await.unwrap();
            sqlx::query(
                "UPDATE webhook_outbox SET status = ?1, attempts = 3, last_status_code = 503, last_error = 'HTTP 503', \
                 finished_at = CASE WHEN ?1 = 'pending' THEN NULL ELSE strftime('%Y-%m-%d %H:%M:%f', 'now') END WHERE generation_id = ?2",
            )
            .bind(status)
            .bind(id)
            .execute(db.pool())
            .await
            .unwrap();
            ids.insert(id, db.webhook_outbox_of(id).await.unwrap().unwrap().id);
        }
        let cooldown = chrono::Duration::seconds(60);
        let delivered = ids["g-delivered"].clone();

        let again = match db.redeliver_webhook_delivery(&key, &delivered, cooldown).await.unwrap() {
            Redelivery::Redelivered(again) => again,
            other => panic!("not redelivered: {other:?}"),
        };
        let state = (again.id.as_str(), again.status.as_str(), again.attempts, again.last_status_code, again.last_error.as_deref());
        assert_eq!(state, (delivered.as_str(), "pending", 0, None, None), "{again:?}");
        assert!(again.finished_at.is_none() && again.claimed_by.is_none(), "{again:?}");
        assert!((again.next_attempt_at - chrono::Utc::now()).num_seconds().abs() < 5, "due now: {again:?}");
        let noted: Option<String> = sqlx::query_scalar("SELECT redelivered_at FROM webhook_outbox WHERE id = ?").bind(&delivered).fetch_one(db.pool()).await.unwrap();
        assert!(noted.is_some(), "when it was redelivered is noted");

        assert!(matches!(db.redeliver_webhook_delivery(&key, &delivered, cooldown).await.unwrap(), Redelivery::Pending(_)), "it is being sent");
        // It ends again at once: within the cooldown it is too soon.
        sqlx::query("UPDATE webhook_outbox SET status = 'delivered', finished_at = strftime('%Y-%m-%d %H:%M:%f', 'now') WHERE id = ?").bind(&delivered).execute(db.pool()).await.unwrap();
        match db.redeliver_webhook_delivery(&key, &delivered, cooldown).await.unwrap() {
            Redelivery::TooSoon { record, retry_at } => {
                assert_eq!(record.status, "delivered");
                let wait = retry_at - chrono::Utc::now();
                assert!(wait > chrono::Duration::seconds(50) && wait <= cooldown, "asked again a minute after the last: {wait:?}");
            }
            other => panic!("not too soon: {other:?}"),
        }
        sqlx::query("UPDATE webhook_outbox SET redelivered_at = strftime('%Y-%m-%d %H:%M:%f', 'now', '-61 seconds') WHERE id = ?").bind(&delivered).execute(db.pool()).await.unwrap();
        assert!(matches!(db.redeliver_webhook_delivery(&key, &delivered, cooldown).await.unwrap(), Redelivery::Redelivered(_)), "the cooldown is over");

        assert!(matches!(db.redeliver_webhook_delivery(&key, &ids["g-failed"], cooldown).await.unwrap(), Redelivery::Redelivered(_)), "a failed one too");
        assert!(matches!(db.redeliver_webhook_delivery(&key, &ids["g-pending"], cooldown).await.unwrap(), Redelivery::Pending(_)));
        assert_eq!(db.redeliver_webhook_delivery(&other, &ids["g-delivered"], cooldown).await.unwrap(), Redelivery::NotFound, "another key's");
        assert_eq!(db.redeliver_webhook_delivery(&key, "whd-unknown", cooldown).await.unwrap(), Redelivery::NotFound);
    }

    // ─── A deleted org's or app's files (migration 031) ─────────────────

    /// A finished generation of `app` (in `org`) with `result_url` and
    /// `meta` as its stored metadata.
    async fn finished_with(db: &SqliteDatabase, id: &str, org: &str, app: &str, result_url: Option<&str>, meta: Option<serde_json::Value>) {
        db.insert_generation(id, None, "mock/x", "mock", "model3d", None, 0.0, Some(org), Some(app)).await.unwrap();
        db.update_generation_if_active(id, "completed", 100, result_url, None, Some(chrono::Utc::now()), meta.as_ref()).await.unwrap();
    }

    /// Every file deletion due now of `of`, claimed, as (scope, tenant, generation, url), sorted.
    async fn claimed_files(db: &SqliteDatabase, of: Option<(&str, &str)>) -> Vec<(String, String, String, String)> {
        let mut got: Vec<_> = db
            .claim_tenant_file_deletions(of, 1000, chrono::Duration::seconds(60))
            .await
            .unwrap()
            .into_iter()
            .map(|d| (d.scope, d.tenant_id, d.generation_id, d.url))
            .collect();
        got.sort();
        got
    }

    /// Deleting an app records, in the transaction that deletes its rows,
    /// each file litegen kept for its generations: an asset's URL unless it
    /// is `kept: false`; a row from before litegen kept results, or whose
    /// metadata isn't JSON, its result_url; nothing for a row that kept
    /// nothing, whose files are deleted already, or that is still running;
    /// nothing of another app's.
    #[tokio::test]
    async fn deleting_an_app_records_the_files_it_kept() {
        let db = in_memory_db().await;
        let org = make_org(&Uuid::new_v4().to_string(), "files-org");
        db.create_organization(&org).await.unwrap();
        let app = make_app(&Uuid::new_v4().to_string(), &org.id, "files-app");
        db.create_application(&app).await.unwrap();
        let other = make_app(&Uuid::new_v4().to_string(), &org.id, "files-other");
        db.create_application(&other).await.unwrap();
        let store = "http://127.0.0.1:4000/v1/models3d/assets/litegen/3d";
        let meta = |files: &[(&str, bool)]| {
            let assets: Vec<serde_json::Value> = files
                .iter()
                .map(|(url, kept)| if *kept { serde_json::json!({ "kind": "mesh", "url": url }) } else { serde_json::json!({ "kind": "mesh", "url": url, "kept": false }) })
                .collect();
            Some(serde_json::json!({ "assets": assets }))
        };
        let (glb, fbx) = (format!("{store}/g-mesh/model.glb"), format!("{store}/g-mesh/model.fbx"));
        let vendor = "https://cdn.vendor.example/preview.png?sig=1";
        finished_with(&db, "g-mesh", &org.id, &app.id, Some(&glb), meta(&[(&glb, true), (&fbx, true), (vendor, false)])).await;
        let legacy = format!("{store}/g-legacy/model.glb");
        finished_with(&db, "g-legacy", &org.id, &app.id, Some(&legacy), None).await;
        finished_with(&db, "g-nothing", &org.id, &app.id, Some(vendor), Some(serde_json::json!({ "keep_outputs": false }))).await;
        let swept = format!("{store}/g-swept/model.glb");
        finished_with(&db, "g-swept", &org.id, &app.id, Some(&swept), meta(&[(&swept, true)])).await;
        db.mark_files_deleted("g-swept", chrono::Utc::now(), "retention").await.unwrap();
        let broken = format!("{store}/g-broken/model.glb");
        finished_with(&db, "g-broken", &org.id, &app.id, None, None).await;
        sqlx::query("UPDATE generations SET metadata = 'not json', result_url = ? WHERE id = 'g-broken'").bind(&broken).execute(db.pool()).await.unwrap();
        db.insert_generation("g-running", None, "mock/x", "mock", "model3d", Some("job"), 0.0, Some(&org.id), Some(&app.id)).await.unwrap();
        let theirs = format!("{store}/g-theirs/model.glb");
        finished_with(&db, "g-theirs", &org.id, &other.id, Some(&theirs), meta(&[(&theirs, true)])).await;

        assert!(db.delete_application(&app.id).await.unwrap());

        let row = |gen: &str, url: &str| ("app".to_string(), app.id.clone(), gen.to_string(), url.to_string());
        let mut want = vec![row("g-mesh", &glb), row("g-mesh", &fbx), row("g-legacy", &legacy), row("g-broken", &broken)];
        want.sort();
        assert_eq!(claimed_files(&db, None).await, want);
        assert!(db.get_generation("g-theirs").await.unwrap().is_some(), "the other app's generation stays");
    }

    /// Deleting an org records the kept files of every app of its, under
    /// the org; another org's stay unrecorded.
    #[tokio::test]
    async fn deleting_an_org_records_the_files_of_all_its_apps() {
        let db = in_memory_db().await;
        let store = "http://127.0.0.1:4000/v1/models3d/assets/litegen/3d";
        let mut orgs = Vec::new();
        for tag in ["going", "staying"] {
            let org = make_org(&Uuid::new_v4().to_string(), &format!("{tag}-org"));
            db.create_organization(&org).await.unwrap();
            for n in 0..2 {
                let app = make_app(&Uuid::new_v4().to_string(), &org.id, &format!("{tag}-app-{n}"));
                db.create_application(&app).await.unwrap();
                let id = format!("g-{tag}-{n}");
                let url = format!("{store}/{id}/model.glb");
                finished_with(&db, &id, &org.id, &app.id, Some(&url), Some(serde_json::json!({ "assets": [{ "kind": "mesh", "url": url }] }))).await;
            }
            orgs.push(org);
        }

        assert!(db.delete_organization(&orgs[0].id).await.unwrap());

        let row = |gen: &str| ("org".to_string(), orgs[0].id.clone(), gen.to_string(), format!("{store}/{gen}/model.glb"));
        assert_eq!(claimed_files(&db, None).await, vec![row("g-going-0"), row("g-going-1")]);
    }

    /// A claim holds what it took: no other claim takes it until its hold has
    /// passed (a worker that died), and then one does, with an attempt more.
    /// A retry from a claim that is no longer the latest is let go; the
    /// latest's makes it due again later, with its error. A finish removes
    /// it. A claim of one app takes only that app's.
    #[tokio::test]
    async fn a_claimed_file_deletion_is_held_until_its_hold_passes() {
        let db = in_memory_db().await;
        let org = make_org(&Uuid::new_v4().to_string(), "held-org");
        db.create_organization(&org).await.unwrap();
        let store = "http://127.0.0.1:4000/v1/models3d/assets/litegen/3d";
        let mut apps = Vec::new();
        for n in 0..2 {
            let app = make_app(&Uuid::new_v4().to_string(), &org.id, &format!("held-app-{n}"));
            db.create_application(&app).await.unwrap();
            let id = format!("g-held-{n}");
            let url = format!("{store}/{id}/model.glb");
            finished_with(&db, &id, &org.id, &app.id, Some(&url), Some(serde_json::json!({ "assets": [{ "kind": "mesh", "url": url }] }))).await;
            assert!(db.delete_application(&app.id).await.unwrap());
            apps.push(app);
        }
        let hold = chrono::Duration::seconds(60);

        let first = db.claim_tenant_file_deletions(Some(("app", &apps[0].id)), 10, hold).await.unwrap();
        assert_eq!(first.iter().map(|d| (d.generation_id.as_str(), d.attempts)).collect::<Vec<_>>(), [("g-held-0", 1)], "only that app's");
        let rest = db.claim_tenant_file_deletions(None, 10, hold).await.unwrap();
        assert_eq!(rest.iter().map(|d| d.generation_id.as_str()).collect::<Vec<_>>(), ["g-held-1"], "the held one isn't taken again");
        assert!(db.claim_tenant_file_deletions(None, 10, hold).await.unwrap().is_empty(), "both held");

        // The holds pass: the worker that held them died.
        sqlx::query("UPDATE tenant_file_deletions SET next_attempt_at = strftime('%Y-%m-%d %H:%M:%f', 'now', '-1 seconds')").execute(db.pool()).await.unwrap();
        let again = db.claim_tenant_file_deletions(None, 1, hold).await.unwrap();
        assert_eq!(again.len(), 1, "at most the limit");
        let d = &again[0];
        assert_eq!(d.attempts, 2, "{d:?}");

        assert!(!db.retry_tenant_file_deletion(d.id, 1, hold, "late").await.unwrap(), "an earlier claim's retry is let go");
        assert!(db.retry_tenant_file_deletion(d.id, 2, chrono::Duration::seconds(600), "S3 DELETE returned HTTP 503").await.unwrap());
        let (error, later): (Option<String>, bool) = sqlx::query_as(
            "SELECT last_error, next_attempt_at > strftime('%Y-%m-%d %H:%M:%f', 'now', '+590 seconds') FROM tenant_file_deletions WHERE id = ?",
        )
        .bind(d.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!((error.as_deref(), later), (Some("S3 DELETE returned HTTP 503"), true));

        let other = db.claim_tenant_file_deletions(None, 10, hold).await.unwrap();
        assert_eq!(other.len(), 1, "the one not retried is due; the retried one waits: {other:?}");
        assert_ne!(other[0].id, d.id);
        assert!(db.finish_tenant_file_deletion(other[0].id).await.unwrap());
        assert!(!db.finish_tenant_file_deletion(other[0].id).await.unwrap(), "gone already");
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tenant_file_deletions").fetch_one(db.pool()).await.unwrap();
        assert_eq!(left, 1, "the retried one waits");
    }

    /// The spend tags of an org's or app's requests (`X-Litegen-Tags`) go
    /// with it: a deleted tenant's tags must not outlive its log rows.
    #[tokio::test]
    async fn deleting_an_app_or_an_org_takes_its_requests_tags() {
        let db = in_memory_db().await;
        for (tag, by_app) in [("app", true), ("org", false)] {
            let org = make_org(&Uuid::new_v4().to_string(), &format!("tags-{tag}-org"));
            db.create_organization(&org).await.unwrap();
            let app = make_app(&Uuid::new_v4().to_string(), &org.id, &format!("tags-{tag}-app"));
            db.create_application(&app).await.unwrap();
            let id = format!("req-tagged-{tag}");
            db.log_request(&id, "mock/x", "mock", "completed", "image", 1.0, 5, None, None, Some(&org.id), Some(&app.id)).await.unwrap();
            let tags = crate::management::types::Attribution { tags: vec!["campaign".into(), format!("client-{tag}")], ..Default::default() };
            db.management().unwrap().attribute_request_log(&id, &tags).await.unwrap();
            let count = || sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM request_log_tags WHERE request_id = ?").bind(id.clone()).fetch_one(db.pool());
            assert_eq!(count().await.unwrap(), 2, "{tag}: tagged");

            if by_app {
                assert!(db.delete_application(&app.id).await.unwrap());
            } else {
                assert!(db.delete_organization(&org.id).await.unwrap());
            }
            assert_eq!(count().await.unwrap(), 0, "deleting the {tag} took its requests' tags");
        }
    }

    /// Request `id`'s log row, with two spend tags and its artifact, all
    /// made `days` days ago.
    async fn logged_request(db: &SqliteDatabase, id: &str, days: i64) {
        db.log_request(id, "mock/x", "mock", "completed", "image", 1.0, 5, None, None, None, None).await.unwrap();
        let tags = crate::management::types::Attribution { tags: vec!["campaign".into(), "client".into()], ..Default::default() };
        db.management().unwrap().attribute_request_log(id, &tags).await.unwrap();
        let artifact = RequestArtifact {
            request_id: id.to_string(),
            media_type: "image".to_string(),
            prompt: Some("a red cube".to_string()),
            negative_prompt: None,
            params_json: None,
            refs_meta_json: None,
            output_kind: "url".to_string(),
            output_value: Some("https://cdn.example/a.png".to_string()),
            output_mime: Some("image/png".to_string()),
            output_truncated: false,
            error_message: None,
            created_at: chrono::Utc::now(),
            org_id: None,
            app_id: None,
        };
        db.insert_request_artifact(&artifact).await.unwrap();
        let ago = format!("-{days} days");
        for stmt in ["UPDATE request_logs SET created_at = datetime('now', ?) WHERE id = ?", "UPDATE request_artifacts SET created_at = datetime('now', ?) WHERE request_id = ?"] {
            sqlx::query(stmt).bind(&ago).bind(id).execute(db.pool()).await.unwrap();
        }
    }

    /// What is left of the request log: its rows, its tags (by request) and
    /// its artifacts, each by request id.
    async fn request_log_left(db: &SqliteDatabase) -> (Vec<String>, Vec<String>, Vec<String>) {
        let ids = |sql: &'static str| sqlx::query_scalar::<_, String>(sql).fetch_all(db.pool());
        (
            ids("SELECT id FROM request_logs ORDER BY id").await.unwrap(),
            ids("SELECT request_id FROM request_log_tags ORDER BY request_id, tag").await.unwrap(),
            ids("SELECT request_id FROM request_artifacts ORDER BY request_id").await.unwrap(),
        )
    }

    /// The request log's purge goes a batch of at most `limit` at a time:
    /// artifacts made before the cutoff on their own (their log rows and tags
    /// stay), and log rows made before it, each with its spend tags. What was
    /// made after the cutoff stays, and so does a generation that shares a
    /// purged log row's id (an async job's).
    #[tokio::test]
    async fn the_request_log_is_purged_a_batch_at_a_time_before_its_cutoff() {
        let db = in_memory_db().await;
        for (id, days) in [("req-old-1", 31), ("req-old-2", 31), ("req-new", 29)] {
            logged_request(&db, id, days).await;
        }
        job_of(&db, "req-old-1", None, "video", Some("job")).await;
        let cutoff = chrono::Utc::now() - chrono::Duration::days(30);
        let all = |ids: &[&str], each: usize| ids.iter().flat_map(|id| std::iter::repeat_n(id.to_string(), each)).collect::<Vec<_>>();

        let mut artifacts = Vec::new();
        for _ in 0..3 {
            artifacts.push(db.purge_request_artifacts(cutoff, 1).await.unwrap());
        }
        assert_eq!(artifacts, [1, 1, 0], "one artifact per batch, until none is due");
        let (logs, tags, kept) = request_log_left(&db).await;
        assert_eq!(kept, ["req-new"], "only the artifact after the cutoff stays");
        assert_eq!(logs, ["req-new", "req-old-1", "req-old-2"], "the log rows stay");
        assert_eq!(tags, all(&["req-new", "req-old-1", "req-old-2"], 2), "their tags stay");

        let mut purged = Vec::new();
        for _ in 0..3 {
            purged.push(db.purge_request_logs(cutoff, 1).await.unwrap());
        }
        assert_eq!(purged, [1, 1, 0], "one log row per batch, until none is due");
        assert_eq!(request_log_left(&db).await, (all(&["req-new"], 1), all(&["req-new"], 2), all(&["req-new"], 1)), "the row after the cutoff stays whole");
        assert!(db.get_generation("req-old-1").await.unwrap().is_some(), "a generation row is never the purge's");
    }

    /// SQLite compares the purge's cutoff with `created_at` as text, so the
    /// two must share one form. A row and an artifact the request log wrote a
    /// moment ago (`datetime('now')`) are after a cutoff two seconds earlier,
    /// so they stay, and before one two seconds later, so they go.
    #[tokio::test]
    async fn the_purge_reads_the_request_logs_own_time_form() {
        let db = in_memory_db().await;
        db.log_request("req-now", "mock/x", "mock", "completed", "image", 1.0, 5, None, None, None, None).await.unwrap();
        let artifact = RequestArtifact {
            request_id: "req-now".to_string(),
            media_type: "image".to_string(),
            prompt: Some("a red cube".to_string()),
            negative_prompt: None,
            params_json: None,
            refs_meta_json: None,
            output_kind: "url".to_string(),
            output_value: None,
            output_mime: None,
            output_truncated: false,
            error_message: None,
            created_at: chrono::Utc::now(),
            org_id: None,
            app_id: None,
        };
        db.insert_request_artifact(&artifact).await.unwrap();
        let earlier = chrono::Utc::now() - chrono::Duration::seconds(2);
        assert_eq!((db.purge_request_artifacts(earlier, 10).await.unwrap(), db.purge_request_logs(earlier, 10).await.unwrap()), (0, 0), "made after the cutoff");
        let later = chrono::Utc::now() + chrono::Duration::seconds(2);
        assert_eq!((db.purge_request_artifacts(later, 10).await.unwrap(), db.purge_request_logs(later, 10).await.unwrap()), (1, 1), "made before the cutoff");
    }

    /// Migration 039: deleting a generation deletes the artifacts of the
    /// cache hits it answered (`output_kind = 'generation'`, its id in
    /// `output_value`) through an index, not a pass over the table.
    #[tokio::test]
    async fn migration_0039_indexes_the_artifacts_a_generation_served() {
        let db = in_memory_db().await;
        let plan: Vec<(i64, i64, i64, String)> =
            sqlx::query_as("EXPLAIN QUERY PLAN DELETE FROM request_artifacts WHERE output_kind = 'generation' AND output_value = ?")
                .bind("g-1")
                .fetch_all(db.pool())
                .await
                .unwrap();
        let plan = plan.into_iter().map(|(_, _, _, detail)| detail).collect::<Vec<_>>().join("; ");
        assert!(plan.contains("USING INDEX idx_request_artifacts_served_generation"), "{plan}");
    }
}
