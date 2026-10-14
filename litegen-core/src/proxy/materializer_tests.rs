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

    const PNG_HEAD: [u8; 12] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
    const JPEG_HEAD: [u8; 4] = [0xFF, 0xD8, 0xFF, 0xE0];

    fn uri_of(out: &MaterializedRequest) -> &str {
        match &out.refs[0].form {
            MaterializedRefForm::Url(u) => u,
            other => panic!("expected a URL form, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn base64_to_url_travels_inline_as_a_data_uri() {
        // Parking it in temp storage minted `local://` URLs no vendor can
        // fetch, and would race an async vendor even with a public bucket.
        let storage = Arc::new(InMemoryStorage::default());
        let schema = schema_with_format(RefProviderFormat::Url);
        let mat = Materializer::new(storage.clone(), reqwest_stub());
        let b64 = base64::engine::general_purpose::STANDARD.encode(PNG_HEAD);
        let out = mat.materialize(&schema, vec![ref_(RefImageKind::Base64, &b64, "init")], &Default::default())
            .await.unwrap();
        assert_eq!(uri_of(&out), format!("data:image/png;base64,{b64}"));
        assert_eq!(storage.uploaded_count(), 0, "nothing is parked in temp storage");
        drop(out); // runs Cleanup, which must find nothing to delete
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(storage.deleted_count(), 0);
    }

    #[tokio::test]
    async fn the_bytes_decide_the_type_over_a_declared_one() {
        let schema = schema_with_format(RefProviderFormat::Url);
        let mat = Materializer::new(Arc::new(InMemoryStorage::default()), reqwest_stub());
        let b64 = base64::engine::general_purpose::STANDARD.encode(PNG_HEAD);
        let value = format!("data:image/jpeg;base64,{b64}");
        let out = mat.materialize(&schema, vec![ref_(RefImageKind::Base64, &value, "init")], &Default::default())
            .await.unwrap();
        assert_eq!(uri_of(&out), format!("data:image/png;base64,{b64}"), "PNG bytes are a PNG, whatever the label says");
    }

    #[tokio::test]
    async fn a_declared_image_type_is_kept_when_the_bytes_are_not_recognised() {
        let schema = schema_with_format(RefProviderFormat::Url);
        let mat = Materializer::new(Arc::new(InMemoryStorage::default()), reqwest_stub());
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"not an image header");
        for (declared, expect) in [("image/webp", "image/webp"), ("text/html", "application/octet-stream")] {
            let value = format!("data:{declared};base64,{b64}");
            let out = mat.materialize(&schema, vec![ref_(RefImageKind::Base64, &value, "init")], &Default::default())
                .await.unwrap();
            assert_eq!(uri_of(&out), format!("data:{expect};base64,{b64}"), "{declared}");
        }
    }

    #[tokio::test]
    async fn invalid_base64_is_refused_rather_than_forwarded() {
        let schema = schema_with_format(RefProviderFormat::Url);
        let mat = Materializer::new(Arc::new(InMemoryStorage::default()), reqwest_stub());
        let err = mat.materialize(&schema, vec![ref_(RefImageKind::Base64, "not base64!!", "init")], &Default::default())
            .await.err().expect("must be refused");
        assert!(matches!(err, MaterializeError::Decode(_)), "{err}");
    }

    #[tokio::test]
    async fn a_blob_part_to_url_travels_inline_with_its_sniffed_type() {
        let schema = schema_with_format(RefProviderFormat::Url);
        let storage = Arc::new(InMemoryStorage::default());
        let mat = Materializer::new(storage.clone(), reqwest_stub());
        let mut ctx = MaterializeContext::default();
        ctx.blob_parts.insert("f1".into(), (bytes::Bytes::from_static(&JPEG_HEAD), "application/octet-stream".into()));
        let out = mat.materialize(&schema, vec![ref_(RefImageKind::Blob, "f1", "init")], &ctx).await.unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(JPEG_HEAD);
        assert_eq!(uri_of(&out), format!("data:image/jpeg;base64,{b64}"));
        assert_eq!(storage.uploaded_count(), 0);
    }

    #[test]
    fn sniffing_recognises_png_jpeg_gif_and_webp_only() {
        assert_eq!(sniff_image_mime(&PNG_HEAD), Some("image/png"));
        assert_eq!(sniff_image_mime(&JPEG_HEAD), Some("image/jpeg"));
        assert_eq!(sniff_image_mime(b"GIF89a....."), Some("image/gif"));
        assert_eq!(sniff_image_mime(b"RIFF\x00\x00\x00\x00WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff_image_mime(b"RIFF\x00\x00\x00\x00WAVEfmt "), None, "RIFF alone is not WebP");
        assert_eq!(sniff_image_mime(b"glTF"), None);
        assert_eq!(sniff_image_mime(&[]), None);
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
