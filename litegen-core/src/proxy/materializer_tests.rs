#[cfg(test)]
mod tests {
    use crate::capabilities::{
        MediaType, ModelCapabilityFlags, ModelPricing, ModelSchema, PromptSpec,
        RefInputSpec, RefProviderFormat, RefRoleSpec,
    };
    use crate::proxy::materializer::*;
    use crate::types::{ReferenceImage, RefImageKind};
    use base64::Engine;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn schema_with_format(form: RefProviderFormat) -> ModelSchema {
        ModelSchema {
            id: "t/m".into(),
            provider: "t".into(),
            media_type: MediaType::Image,
            display_name: "M".into(),
            description: "".into(),
            pricing: ModelPricing { base_cost_usd: 0.01, variable_pricing: None },
            capabilities: ModelCapabilityFlags { text_to_image: true, ..Default::default() },
            prompt: PromptSpec { required: true, min_length: None, max_length: None },
            params: Default::default(),
            ref_inputs: Some(RefInputSpec {
                max_total: 2,
                default_role: Some("init".into()),
                provider_format: form,
                roles: HashMap::from([
                    ("init".into(), RefRoleSpec { required: false, min_count: 0, max_count: 1 }),
                    ("mask".into(), RefRoleSpec { required: false, min_count: 0, max_count: 1 }),
                ]),
            }),
            extra_allowlist: vec![],
            tags: vec![],
        }
    }

    fn ref_(kind: RefImageKind, value: &str, role: &str) -> ReferenceImage {
        ReferenceImage { kind, value: value.into(), role: Some(role.into()) }
    }

    #[tokio::test]
    async fn passthrough_url_to_url() {
        let schema = schema_with_format(RefProviderFormat::Url);
        let mat = Materializer::new(Arc::new(InMemoryStorage::default()), reqwest_stub());
        let refs = vec![ref_(RefImageKind::Url, "https://x/a.png", "init")];
        let out = mat.materialize(&schema, refs, &Default::default()).await.unwrap();
        assert_eq!(out.refs.len(), 1);
        match &out.refs[0].form {
            MaterializedRefForm::Url(u) => assert_eq!(u, "https://x/a.png"),
            other => panic!("expected URL, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn base64_to_url_uploads() {
        let storage = Arc::new(InMemoryStorage::default());
        let schema = schema_with_format(RefProviderFormat::Url);
        let mat = Materializer::new(storage.clone(), reqwest_stub());
        let refs = vec![ref_(RefImageKind::Base64, &base64::engine::general_purpose::STANDARD.encode(b"hello"), "init")];
        let out = mat.materialize(&schema, refs, &Default::default()).await.unwrap();
        assert!(matches!(out.refs[0].form, MaterializedRefForm::Url(_)));
        assert_eq!(storage.uploaded_count(), 1);
        drop(out); // runs Cleanup
        // Spawned cleanup task is async; give it a tick.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(storage.deleted_count(), 1);
    }

    #[tokio::test]
    async fn base64_to_base64_passthrough() {
        let schema = schema_with_format(RefProviderFormat::Base64);
        let mat = Materializer::new(Arc::new(InMemoryStorage::default()), reqwest_stub());
        let refs = vec![ref_(RefImageKind::Base64, "ZGF0YQ==", "init")];
        let out = mat.materialize(&schema, refs, &Default::default()).await.unwrap();
        assert!(matches!(out.refs[0].form, MaterializedRefForm::Base64(_)));
    }

    #[tokio::test]
    async fn base64_to_multipart_decodes() {
        use std::collections::HashMap;
        let schema = schema_with_format(RefProviderFormat::Multipart(
            crate::capabilities::RefProviderFormatMultipart {
                field_map: HashMap::from([("init".into(), "image".into())]),
            },
        ));
        let mat = Materializer::new(Arc::new(InMemoryStorage::default()), reqwest_stub());
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"hello");
        let refs = vec![ref_(RefImageKind::Base64, &b64, "init")];
        let out = mat.materialize(&schema, refs, &Default::default()).await.unwrap();
        match &out.refs[0].form {
            MaterializedRefForm::MultipartField { field_name, bytes, .. } => {
                assert_eq!(field_name, "image");
                assert_eq!(bytes.as_ref(), b"hello");
            }
            other => panic!("expected multipart, got {:?}", other),
        }
    }

    // Helper: an in-memory fake of the storage backend.
    #[derive(Default)]
    pub struct InMemoryStorage {
        uploaded: std::sync::Mutex<Vec<String>>,
        deleted: std::sync::Mutex<Vec<String>>,
    }
    impl InMemoryStorage {
        pub fn uploaded_count(&self) -> usize { self.uploaded.lock().unwrap().len() }
        pub fn deleted_count(&self) -> usize { self.deleted.lock().unwrap().len() }
    }
    #[async_trait::async_trait]
    impl TempStorage for InMemoryStorage {
        async fn put(&self, key: &str, _bytes: bytes::Bytes, _ct: &str) -> Result<String, MaterializeError> {
            self.uploaded.lock().unwrap().push(key.to_string());
            Ok(format!("https://fake/{}", key))
        }
        async fn delete(&self, key: &str) -> Result<(), MaterializeError> {
            self.deleted.lock().unwrap().push(key.to_string());
            Ok(())
        }
    }

    fn reqwest_stub() -> reqwest::Client { reqwest::Client::new() }
}
