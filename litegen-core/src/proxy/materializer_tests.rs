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
            task: Default::default(),
            native_orientation: None,
            preserves_unmasked: None,
            display_name: "M".into(),
            description: "".into(),
            docs_url: None,
            pricing: ModelPricing { base_cost_usd: 0.01, variable_pricing: Vec::new() },
            capabilities: ModelCapabilityFlags { text_to_image: true, ..Default::default() },
            prompt: PromptSpec { required: true, min_length: None, max_length: None, required_for_text_to_video: false },
            params: Default::default(),
            ref_inputs: Some(RefInputSpec {
                max_total: 2,
                default_role: Some("init".into()),
                provider_format: form,
                data_uri: true,
                view_order: Vec::new(),
                roles: HashMap::from([
                    ("init".into(), RefRoleSpec { required: false, min_count: 0, max_count: 1, convention: None, native_grow: false, timing: None, time_step_s: None, max_time_s: None }),
                    ("mask".into(), RefRoleSpec { required: false, min_count: 0, max_count: 1, convention: None, native_grow: false, timing: None, time_step_s: None, max_time_s: None }),
                ]),
            }),
            extra_allowlist: vec![],
            extra_params: Default::default(),
            litegen_params: Default::default(),
            tags: vec![],
            deprecated_at: None, retires_at: None, replaced_by: None, status: Default::default(),
        }
    }

    fn ref_(kind: RefImageKind, value: &str, role: &str) -> ReferenceImage {
        ReferenceImage { kind, value: value.into(), role: Some(role.into()), time_s: None }
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

    /// P15 (timing; run alone): decoding large base64 references for a
    /// multipart vendor holds no async worker. Today a task queued behind
    /// the decode waited as long as it took.
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn decoding_a_large_reference_leaves_the_runtime_free() {
        use std::collections::HashMap;
        let schema = schema_with_format(RefProviderFormat::Multipart(crate::capabilities::RefProviderFormatMultipart {
            field_map: HashMap::from([("init".into(), "image".into()), ("mask".into(), "mask".into())]),
        }));
        let raw = vec![7u8; 15 * 1024 * 1024];
        let b64 = base64::engine::general_purpose::STANDARD.encode(&raw);
        // The materializer converts what the validator let through; four
        // references make the decode heavy enough to measure in a debug build.
        let refs: Vec<_> = ["init", "mask", "init", "mask"].iter().map(|role| ref_(RefImageKind::Base64, &b64, role)).collect();
        let mat = Arc::new(Materializer::new(Arc::new(InMemoryStorage::default()), reqwest_stub()));
        let started = std::time::Instant::now();
        mat.materialize(&schema, refs.clone(), &Default::default()).await.unwrap();
        let alone = started.elapsed();
        assert!(alone >= std::time::Duration::from_millis(200), "too light to show a stall: {alone:?}");
        let (out, stall) = crate::util::blocking::longest_stall_while({
            let mat = mat.clone();
            async move { mat.materialize(&schema, refs, &Default::default()).await.unwrap() }
        })
        .await;
        match &out.refs[0].form {
            MaterializedRefForm::MultipartField { bytes, .. } => assert_eq!(bytes.len(), raw.len()),
            other => panic!("expected multipart, got {other:?}"),
        }
        assert!(stall < alone / 4, "the runtime stalled {stall:?} while decoding (the decode alone takes {alone:?})");
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

    // ─── L4: a vendor that refuses data URIs gets a published URL ───────────

    fn url_only_schema() -> ModelSchema {
        let mut s = schema_with_format(RefProviderFormat::Url);
        s.ref_inputs.as_mut().unwrap().data_uri = false;
        s
    }

    fn publisher(base: &str) -> (Arc<crate::proxy::ref_publish::RefPublisher>, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap().keep();
        let store: Arc<dyn crate::store::ImageStorage> =
            Arc::new(crate::store::LocalModel3dStorage::new(dir.clone(), base.into()));
        (Arc::new(crate::proxy::ref_publish::RefPublisher::new(store, base.into())), dir)
    }

    #[tokio::test]
    async fn a_base64_ref_for_a_url_only_vendor_is_published_at_an_unguessable_url() {
        let (p, _dir) = publisher("https://litegen.example.com");
        let mat = Materializer::new(Arc::new(InMemoryStorage::default()), reqwest_stub()).with_publisher(p.clone());
        let png = [PNG_HEAD.as_slice(), b"rest-of-the-image"].concat();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let out = mat.materialize(&url_only_schema(), vec![ref_(RefImageKind::Base64, &b64, "init")], &Default::default()).await.unwrap();
        let MaterializedRefForm::Url(url) = &out.refs[0].form else { panic!("{:?}", out.refs[0].form) };
        let token = url.strip_prefix("https://litegen.example.com/v1/refs/").unwrap_or_else(|| panic!("{url}"));
        assert!(token.len() >= 40, "unguessable: {token}");
        let token = token.to_string();
        let (bytes, ct) = p.get(&token).await.unwrap().expect("served while the job runs");
        assert_eq!((bytes.as_ref(), ct.as_str()), (png.as_slice(), "image/png"));
        drop(out);
        assert!(p.get(&token).await.unwrap().is_some(), "kept after the request returns: an async vendor fetches later");
    }

    #[tokio::test]
    async fn a_vendor_that_takes_data_uris_still_gets_one() {
        let (p, _dir) = publisher("https://litegen.example.com");
        let mat = Materializer::new(Arc::new(InMemoryStorage::default()), reqwest_stub()).with_publisher(p);
        let b64 = base64::engine::general_purpose::STANDARD.encode([PNG_HEAD.as_slice(), b"x"].concat());
        let out = mat
            .materialize(&schema_with_format(RefProviderFormat::Url), vec![ref_(RefImageKind::Base64, &b64, "init")], &Default::default())
            .await
            .unwrap();
        let MaterializedRefForm::Url(url) = &out.refs[0].form else { panic!() };
        assert!(url.starts_with("data:image/png;base64,"), "{url}");
    }

    #[tokio::test]
    async fn without_a_public_address_a_url_only_vendor_is_refused_before_anything_runs() {
        for base in ["http://127.0.0.1:4000", "http://localhost:4000", "http://192.168.1.20:4000"] {
            let (p, _dir) = publisher(base);
            let mat = Materializer::new(Arc::new(InMemoryStorage::default()), reqwest_stub()).with_publisher(p);
            let b64 = base64::engine::general_purpose::STANDARD.encode(PNG_HEAD);
            let Err(err) = mat.materialize(&url_only_schema(), vec![ref_(RefImageKind::Base64, &b64, "init")], &Default::default()).await else {
                panic!("{base}: published to an address no vendor reaches");
            };
            assert!(err.to_string().contains("public"), "{base}: {err}");
        }
    }
}
