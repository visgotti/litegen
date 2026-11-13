#[cfg(test)]
mod tests {
    use uuid::Uuid;
    use crate::db::sqlite::SqliteDatabase;
    use crate::db::trait_def::DatabaseStore;
    use crate::types::{AuditLogEntry, AuditLogFilter, GenerationStatus, Invitation, PasswordReset, RequestArtifact, Role, Session, UpdateApiKeyRequest, User};
    use crate::db::sqlite::compute_percentiles;
    use crate::auth::tokens::{generate_csrf_token, generate_session_token};

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
            token_quota: Some(5.0),
            rpm_limit: Some(30),
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
        let got = db.get_session(&s.id).await.unwrap().unwrap();
        assert_eq!(got.user_id, user.id);
        assert_eq!(got.csrf_token, s.csrf_token);
        let new_exp = chrono::Utc::now() + chrono::Duration::days(14);
        db.bump_session_expiry(&s.id, new_exp).await.unwrap();
        let bumped = db.get_session(&s.id).await.unwrap().unwrap();
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
        // delete all except sess1
        let deleted = db.delete_user_sessions(&user.id, Some(&sess1.id)).await.unwrap();
        assert_eq!(deleted, 1, "should have deleted 1 session");
        assert!(db.get_session(&sess1.id).await.unwrap().is_some(), "sess1 should survive");
        assert!(db.get_session(&sess2.id).await.unwrap().is_none(), "sess2 should be deleted");
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
        assert!(matches!(db.idempotency_begin("org:a", "k1", "hash-1").await.unwrap(), IdempotencyBegin::Started));
        assert!(matches!(db.idempotency_begin("org:a", "k1", "hash-1").await.unwrap(), IdempotencyBegin::InProgress));
        db.idempotency_complete("org:a", "k1", &stored(200, r#"{"id":"litegen-3d-x"}"#)).await.unwrap();
        match db.idempotency_begin("org:a", "k1", "hash-1").await.unwrap() {
            IdempotencyBegin::Replay(r) => {
                assert_eq!(r.status, 200);
                assert_eq!(r.body, br#"{"id":"litegen-3d-x"}"#.to_vec());
                assert_eq!(r.headers, vec![("content-type".to_string(), "application/json".to_string())]);
            }
            other => panic!("expected a replay, got {other:?}"),
        }
        assert!(matches!(db.idempotency_begin("org:a", "k1", "hash-2").await.unwrap(), IdempotencyBegin::Conflict));
        // Another owner's key of the same spelling is its own record.
        assert!(matches!(db.idempotency_begin("org:b", "k1", "hash-2").await.unwrap(), IdempotencyBegin::Started));
    }

    #[tokio::test]
    async fn a_released_key_can_run_again() {
        let db = in_memory_db().await;
        assert!(matches!(db.idempotency_begin("s", "k", "h").await.unwrap(), IdempotencyBegin::Started));
        db.idempotency_release("s", "k").await.unwrap();
        assert!(matches!(db.idempotency_begin("s", "k", "h2").await.unwrap(), IdempotencyBegin::Started));
    }

    #[tokio::test]
    async fn a_record_older_than_a_day_is_forgotten_and_a_stale_run_abandoned() {
        let db = in_memory_db().await;
        db.idempotency_begin("s", "old", "h").await.unwrap();
        db.idempotency_complete("s", "old", &stored(200, "{}")).await.unwrap();
        db.idempotency_begin("s", "stuck", "h").await.unwrap();
        let day_ago = chrono::Utc::now() - chrono::Duration::hours(25);
        let quarter_ago = chrono::Utc::now() - chrono::Duration::minutes(16);
        sqlx::query("UPDATE idempotency_keys SET created_at = ? WHERE idem_key = 'old'").bind(day_ago).execute(db.pool()).await.unwrap();
        sqlx::query("UPDATE idempotency_keys SET created_at = ? WHERE idem_key = 'stuck'").bind(quarter_ago).execute(db.pool()).await.unwrap();
        // Past 24 h the key is free again, whatever body comes with it.
        assert!(matches!(db.idempotency_begin("s", "old", "another").await.unwrap(), IdempotencyBegin::Started));
        // A run that never finished within 15 minutes died with its process.
        assert!(matches!(db.idempotency_begin("s", "stuck", "h").await.unwrap(), IdempotencyBegin::Started));
    }

    #[tokio::test]
    async fn racing_first_requests_start_exactly_once() {
        let db = std::sync::Arc::new(in_memory_db().await);
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let db = db.clone();
            tasks.push(tokio::spawn(async move { db.idempotency_begin("s", "race", "h").await.unwrap() }));
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
        let begun = db.idempotency_begin("s", "k", "h").await.unwrap();
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
}
