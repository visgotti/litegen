#[cfg(test)]
mod tests {
    use litegen::providers::{parse_api_keys, ApiKeyPool, apply_markup, usd_to_tokens, build_cost_estimate};
    use litegen::types::*;

    // ─── API Key Pool Tests ─────────────────────────────────────────────

    #[test]
    fn test_parse_api_keys_single() {
        let keys = parse_api_keys("sk-abc123");
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, "sk-abc123");
        assert_eq!(keys[0].weight, 1);
    }

    #[test]
    fn test_parse_api_keys_multiple_with_weights() {
        let keys = parse_api_keys("sk-key1:3,sk-key2:1,sk-key3:2");
        assert_eq!(keys.len(), 3);
        assert_eq!(keys[0].key, "sk-key1");
        assert_eq!(keys[0].weight, 3);
        assert_eq!(keys[1].key, "sk-key2");
        assert_eq!(keys[1].weight, 1);
        assert_eq!(keys[2].key, "sk-key3");
        assert_eq!(keys[2].weight, 2);
    }

    #[test]
    fn test_parse_api_keys_empty() {
        let keys = parse_api_keys("");
        assert!(keys.is_empty());
    }

    #[test]
    fn test_api_key_pool_round_robin() {
        let entries = vec![
            ApiKeyEntry { key: "key-a".into(), weight: 1, label: None },
            ApiKeyEntry { key: "key-b".into(), weight: 1, label: None },
        ];
        let pool = ApiKeyPool::new(entries);
        let k1 = pool.next().to_string();
        let k2 = pool.next().to_string();
        let k3 = pool.next().to_string();
        // Should cycle
        assert_ne!(k1, k2);
        assert_eq!(k1, k3);
    }

    #[test]
    fn test_api_key_pool_weighted() {
        let entries = vec![
            ApiKeyEntry { key: "key-a".into(), weight: 3, label: None },
            ApiKeyEntry { key: "key-b".into(), weight: 1, label: None },
        ];
        let pool = ApiKeyPool::new(entries);
        // With weight 3:1, schedule is [a, a, a, b]
        let mut a_count = 0;
        let mut b_count = 0;
        for _ in 0..8 {
            let key = pool.next();
            if key == "key-a" { a_count += 1; }
            if key == "key-b" { b_count += 1; }
        }
        // Should have ~3:1 ratio (exactly, since schedule repeats)
        assert_eq!(a_count, 6); // 3/4 of 8
        assert_eq!(b_count, 2); // 1/4 of 8
    }

    // ─── Cost Calculation Tests ─────────────────────────────────────────

    #[test]
    fn test_apply_markup_zero() {
        let (markup, total) = apply_markup(0.10, 0.0);
        assert_eq!(markup, 0.0);
        assert!((total - 0.10).abs() < 1e-10);
    }

    #[test]
    fn test_apply_markup_20_percent() {
        let (markup, total) = apply_markup(1.00, 20.0);
        assert!((markup - 0.20).abs() < 1e-10);
        assert!((total - 1.20).abs() < 1e-10);
    }

    #[test]
    fn test_usd_to_tokens() {
        // $0.04 at $0.001/token = 40 tokens
        let tokens = usd_to_tokens(0.04, 0.001);
        assert_eq!(tokens, 40);
    }

    #[test]
    fn test_usd_to_tokens_rounds_up() {
        // $0.0015 at $0.001/token = ceil(1.5) = 2 tokens
        let tokens = usd_to_tokens(0.0015, 0.001);
        assert_eq!(tokens, 2);
    }

    #[test]
    fn test_build_cost_estimate() {
        let est = build_cost_estimate(0.10, 10.0, CostSource::Estimated, None);
        assert!((est.base_cost_usd - 0.10).abs() < 1e-10);
        assert!((est.markup_usd - 0.01).abs() < 1e-10);
        assert!((est.total_cost_usd - 0.11).abs() < 1e-10);
        assert!(est.tokens_required > 0);
    }

    // ─── Type Serialization Tests ───────────────────────────────────────

    #[test]
    fn test_image_request_deserialization() {
        let json = r#"{
            "prompt": "a cat",
            "model": "openai/dall-e-3"
        }"#;
        let req: ImageGenerationRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.base.prompt, "a cat");
        assert_eq!(req.base.model, "openai/dall-e-3");
        assert_eq!(req.base.n, 1); // default
        assert_eq!(req.response_format, "url"); // default
    }

    #[test]
    fn test_video_request_deserialization() {
        let json = r#"{
            "prompt": "a flying car",
            "model": "runway/gen-3",
            "duration_seconds": 10.0,
            "aspect_ratio": "16:9"
        }"#;
        let req: VideoGenerationRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.base.prompt, "a flying car");
        assert_eq!(req.base.model, "runway/gen-3");
        assert_eq!(req.duration_seconds, 10.0);
        assert_eq!(req.aspect_ratio.unwrap(), "16:9");
    }

    #[test]
    fn test_generation_status_serialization() {
        let status = GenerationStatus::Completed;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"completed\"");
    }

    #[test]
    fn test_routing_strategy_default() {
        let strategy = RoutingStrategy::default();
        assert_eq!(strategy, RoutingStrategy::Fallback);
    }

    // ─── Configuration Tests ────────────────────────────────────────────

    #[test]
    fn test_default_config() {
        let config = litegen::config::AppConfig::default();
        assert_eq!(config.server.port, 4000);
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.database_url, "sqlite://litegen.db");
        assert!(!config.cache.enabled);
    }

    // ─── Route Matching Tests ───────────────────────────────────────────

    #[test]
    fn test_model_route_matching() {
        // Test exact match
        assert!(route_matches("dall-e-3", "dall-e-3"));
        assert!(!route_matches("dall-e-3", "dall-e-2"));

        // Test wildcard
        assert!(route_matches("*", "anything"));

        // Test glob
        assert!(route_matches("openai/*", "openai/dall-e-3"));
        assert!(!route_matches("openai/*", "stability/sdxl"));
    }

    fn route_matches(pattern: &str, model: &str) -> bool {
        if pattern == "*" { return true; }
        if let Some(prefix) = pattern.strip_suffix("/*") {
            return model.starts_with(prefix);
        }
        pattern.eq_ignore_ascii_case(model)
    }
}
