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
        ).await.unwrap();

        let gen = db.get_generation("litegen-vid-test-2").await.unwrap().unwrap();
        assert_eq!(gen.key_id, Some(key.id), "key_id should match");
        assert!((gen.cost_usd - 0.10).abs() < 1e-9);
    }

    #[tokio::test]
    async fn update_generation_status_to_completed() {
        let db = in_memory_db().await;
        db.insert_generation("litegen-vid-upd-1", None, "mock/video-gen", "mock", "video", Some("j1"), 0.0).await.unwrap();

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
        db.insert_generation("litegen-vid-fail-1", None, "mock/video-gen", "mock", "video", Some("j2"), 0.0).await.unwrap();

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

        db.insert_generation("litegen-vid-act-1", None, "mock/video-gen", "mock", "video", Some("j1"), 0.0).await.unwrap();
        db.insert_generation("litegen-vid-act-2", None, "mock/video-gen", "mock", "video", Some("j2"), 0.0).await.unwrap();
        db.insert_generation("litegen-vid-act-3", None, "mock/video-gen", "mock", "video", Some("j3"), 0.0).await.unwrap();

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
        db.insert_generation("lg-gen-1", Some(&key1.id), "mock/v", "mock", "video", None, 0.0).await.unwrap();
        db.insert_generation("lg-gen-2", Some(&key2.id), "mock/v", "mock", "video", None, 0.0).await.unwrap();
        db.insert_generation("lg-gen-3", None, "mock/v", "mock", "video", None, 0.0).await.unwrap();

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
        db.insert_generation("lg-all-1", Some(&key1.id), "mock/v", "mock", "video", None, 0.0).await.unwrap();
        db.insert_generation("lg-all-2", None, "mock/v", "mock", "video", None, 0.0).await.unwrap();

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
        db.insert_generation("lg-cancel-1", None, "mock/v", "mock", "video", None, 0.0).await.unwrap();

        let result = db.cancel_generation("lg-cancel-1").await.unwrap();
        assert!(result.is_some(), "cancel should return the updated row");
        let gen = result.unwrap();
        assert_eq!(gen.status, GenerationStatus::Cancelled);
        assert!(gen.completed_at.is_some());
    }

    #[tokio::test]
    async fn cancel_generation_on_completed_returns_none() {
        let db = in_memory_db().await;
        db.insert_generation("lg-cancel-2", None, "mock/v", "mock", "video", None, 0.0).await.unwrap();
        db.update_generation_status("lg-cancel-2", "completed", 100, None, None, Some(chrono::Utc::now())).await.unwrap();

        let result = db.cancel_generation("lg-cancel-2").await.unwrap();
        assert!(result.is_none(), "cancel on completed should return None (409 condition)");
    }

    // ─── get_request_logs_filtered ────────────────────────────────────────────

    #[tokio::test]
    async fn logs_filtered_by_model() {
        let db = in_memory_db().await;
        db.log_request("id1", "openai/dall-e-3", "openai", "completed", "image", 0.01, 100, None, None).await.unwrap();
        db.log_request("id2", "mock/image-gen", "mock", "completed", "image", 0.0, 50, None, None).await.unwrap();
        db.log_request("id3", "mock/image-gen", "mock", "failed", "image", 0.0, 20, Some("err"), None).await.unwrap();

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
        db.log_request("s1", "openai/dall-e-3", "openai", "completed", "image", 0.01, 100, None, None).await.unwrap();
        db.log_request("s2", "mock/image-gen", "mock", "failed", "image", 0.0, 50, Some("err"), None).await.unwrap();
        db.log_request("s3", "mock/image-gen", "mock", "completed", "image", 0.0, 50, None, None).await.unwrap();

        let filters = crate::types::LogFilters {
            status: Some("failed".to_string()),
            ..Default::default()
        };
        let (logs, total) = db.get_request_logs_filtered(&filters, 1, 50).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(logs[0].id, "s2");
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
            db.log_request(id, "mock/v", "mock", "completed", "image", 0.0, lat, None, None).await.unwrap();
        }
        // 3 failed requests with very high latency that should NOT affect percentiles
        for (id, lat) in [("pf6", 10000i64), ("pf7", 20000), ("pf8", 30000)] {
            db.log_request(id, "mock/v", "mock", "failed", "image", 0.0, lat, Some("err"), None).await.unwrap();
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

        // All four new tables must exist and be queryable.
        for table in ["organizations", "applications", "organization_members", "provider_credentials"] {
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
}
