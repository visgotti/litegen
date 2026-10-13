# Auth and credentials

The user configures provider keys in the dashboard, or through the management
API, and expects every model of that provider to work right away, in every
family. This file explains what makes that true.

## Contents

- [Schemes](#schemes)
- [The adapter pattern](#the-adapter-pattern)
- [How a dashboard key reaches your adapter](#how-a-dashboard-key-reaches-your-adapter)
- [Adding a scheme or a credential field](#adding-a-scheme-or-a-credential-field)
- [Tests that prove it just works](#tests-that-prove-it-just-works)

## Schemes

Auth is a property of the vendor. It is declared in code
(`litegen-core/src/providers/auth/mod.rs`) and is never user-configurable;
users supply only credentials.

| `AuthSpec` | On the wire | `required_fields` | Dashboard `pool_field` | Used today by |
|---|---|---|---|---|
| `AuthSpec::bearer()` | `Authorization: Bearer <key>` | `api_key` | `api_keys` | luma image, runway image, recraft, minimax, bytedance, leonardo |
| `AuthSpec::raw_header("<name>")` | `<name>: <key>` | `api_key` | `api_keys` | bfl `x-key`, ideogram `Api-Key`, pixverse `API-KEY`, google video `x-goog-api-key` |
| `AuthSpec::Header { name, value_prefix }` | `<name>: <prefix><key>` | `api_key` | `api_keys` | vidu `Authorization: Token <key>`. fal needs `Authorization: Key <key>`, which its adapters hand-write today |
| `AuthSpec::QueryParam { name }` | `?<name>=<key>` | `api_key` | `api_keys` | google image `?key=` (hand-written today) |
| `AuthSpec::AwsSigV4 { service, default_region }` | SigV4 over the body bytes | `key_id`, `key_secret`, `region` | `credential_sets` | bedrock |
| `AuthSpec::KlingJwt` | an HS256 JWT minted per request | `key_id`, `key_secret` | `credential_sets` | kling |
| `AuthSpec::TencentTc3 { service, default_region }` | TC3-HMAC-SHA256 over the body | `key_id`, `key_secret`, `region` | `credential_sets` | hunyuan |

`apply()` handles the `Header`, `QueryParam` and `KlingJwt` schemes. The
signing schemes need the finalized body bytes, so they go through
`sigv4::sign` or `tc3::sign` instead, and `apply()` returns an error if you
pass it one. A region is interpolated into hosts such as
`bedrock-runtime.{region}.amazonaws.com`, and BYO credentials are
tenant-controlled, so normalize it the way `normalize_aws_region` does: an
injected host is a security bug.

**Legacy adapters hand-write their auth:** openai (image and video),
stability, replicate (image and video), fal (image, video and 3D), luma video,
runway video and google image. Don't copy them. If you change one, migrating
it to `AuthSpec` is welcome, behind a test that asserts the header. A vendor's
image and video adapters can disagree today: Luma image uses `AuthSpec` while
Luma video hand-writes the same header. When you add a family, match the
spec, not the legacy sibling's code.

## The adapter pattern

This is the shape every post-audit adapter uses (for example
`image/bfl.rs` and `video/vidu.rs`):

```rust
pub struct AcmeProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,          // signing vendors: Option<CredentialPool>
    auth: AuthSpec,
    client: Client,
}

impl AcmeProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::bearer(),     // the vendor's documented scheme
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    /// The one place a base URL comes from.
    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://api.acme.example")   // documented production base
    }

    /// The credential for THIS request: the next pooled key when a pool
    /// exists, else the single configured credential.
    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("acme".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
            // signing: Ok(base.with_signing(pool.next()))
        }
        Ok(base)
    }
}

// in `impl ImageProvider / VideoProvider / Model3dProvider for AcmeProvider`:
fn configure(&mut self, config: ProviderInstanceConfig) {
    // Guarded: `ApiKeyPool::shared` panics on an empty list, and configs with
    // no pool are normal (a single key, a default config in tests).
    if !config.api_keys.is_empty() {
        self.key_pool = Some(ApiKeyPool::shared(config.api_keys.clone()));
    }
    // signing: if !config.credentials.credential_sets.is_empty() {
    //     self.key_pool = Some(CredentialPool::shared(config.credentials.credential_sets.clone()));
    // }
    self.config = Some(config);
}

fn is_configured(&self) -> bool {
    self.config
        .as_ref()
        .is_some_and(|c| self.auth.is_satisfied_by(&c.credentials) || self.key_pool.is_some())
}

// each request:
let creds = self.creds()?;
let builder = crate::providers::auth::apply(
    &self.auth,
    &creds,
    self.client.post(&url).json(&body),
)?;
let resp = crate::providers::inject_trace_headers(builder).send().await /* map_err… */;
```

Notes:

- Use `ApiKeyPool::shared` / `CredentialPool::shared` on the request path,
  not `::new`. The shared constructors keep the rotation cursor process-wide,
  keyed by a credential fingerprint, so rotation survives the per-request
  rebuilds that BYO causes. With `::new` a weighted pool would always return
  its first key.
- Vendor-specific auxiliary values, such as Bedrock video's `s3_output_uri`,
  arrive in `ProviderCredentials.extra` (any other string key in the stored
  JSON) or in `ProviderInstanceConfig.options`. Read both if you need one,
  the way `video/bedrock.rs` does.
- Read keys through `config.credentials` (via `creds()` and the pool), never
  through `config.api_key`. The three config sources fill different fields:
  YAML and env fill both; BYO fills `credentials` and `api_keys`; the live-test
  helper `api_key_cfg` fills `credentials` only. An adapter that reads
  `config.api_key` reports itself unconfigured in live tests and silently
  ignores a pool-only BYO key.
- `ProviderInstanceConfig.extra_headers` is not applied by any adapter today.
  Don't depend on it.
- Never attach credentials to a vendor's signed download URL. Those URLs
  often point at a CDN or object store that isn't the vendor's API host.
  Send auth there only when the vendor documents that it's required, as
  Veo's `video.uri` does.

## How a dashboard key reaches your adapter

1. The Providers page renders one form per entry of `GET /v1/providers`
   (`provider_catalog()`): the name, its `modalities` (derived from
   `IMAGE_PROVIDERS`, `VIDEO_PROVIDERS` and `MODEL3D_PROVIDERS`), `auth_scheme`,
   `pool_field` and `fields`.
2. Saving the form posts `{"provider":"<p>","credentials":{"<pool_field>":[…entries]}}`
   to `POST /v1/orgs/{org_id}/provider-credentials`. The body is encrypted
   (AES-256-GCM, with a `secrets_key` required) and stored per org. Entries
   look like `{"key","weight","label"}` for `api_keys`, or
   `{"key_id","key_secret","region"?,"weight","label"}` for `credential_sets`.
   There is usually no single `api_key`.
3. At request time, `resolve_org_provider_credential` decrypts the entry and
   `ProviderCredentials::from_json` parses it. The registry's
   `*_provider_for_request(name, Some(creds))` then builds a **fresh** adapter
   from the registered global config (or a default one) via
   `with_credentials`. That keeps `api_base`, `model_mapping` and `options`
   but swaps the credentials, so BYO works even when the platform has no key
   for the vendor.
4. The background poller resolves the same credential for each in-flight row
   and builds the same fresh instance to poll. Client polls work differently
   by family. 3D client polls reuse the instance that submitted the job
   (`Model3dJob.provider`), so they carry the BYO key. Video client polls use
   the platform instance (`get_video_status` calls `video_provider_for`);
   see known-gaps.md.
5. Platform keys come from `litegen.yaml` (`providers.<p>`) or env vars mapped
   in `load_provider_env_overrides`. `init_from_config` skips a provider
   unless `credentials.any_present()` (the mock provider is exempt).

So for "it just works":

- the provider name is in the right `*_PROVIDERS` list(s), with a build arm;
- `provider_catalog()` describes the right fields (automatic for single-key
  vendors);
- `configure()` builds a pool from `api_keys` or `credential_sets`;
- `is_configured()` accepts a pool-only config;
- nothing in the adapter reads `config.api_key` directly without going
  through `creds()` or the pool. A pool-only BYO config has an empty
  `api_key`.

## Adding a scheme or a credential field

- **New scheme:** add an `AuthSpec` variant with `scheme_name()`,
  `required_fields()` and `is_satisfied_by()`, then its application (an arm in
  `apply`, or a signer module next to `sigv4.rs` and `tc3.rs`). Unit-test it
  against the vendor's own documented example (signature or token) so you
  aren't only checking your code against itself. Add a `provider_catalog()`
  arm and update `provider_catalog_covers_registry` if its assertions
  enumerate schemes.
- **A field beyond the scheme's**, such as an account or group id: the
  dashboard renders only the `fields` that `provider_catalog()` declares, and
  `from_json` puts unknown string keys in `extra`. Adding one is a core change
  (registry, dashboard form expectations, docs), so ask the user first.

## Tests that prove it just works

- **Adapter:** a wiremock test that configures only `api_keys` (no
  `api_key`) and asserts that the auth header carries the pooled key. For
  signing vendors, assert the header shape (`Authorization` with the
  algorithm token, the date header) using a fixed credential.
- **Adapter:** `is_configured()` is false on `new()` and true on a
  pool-only config.
- **Registry:**
  `<family>_provider_for_request("<p>", Some(ProviderCredentials::from_json(&json!({"api_keys":[{"key":"k"}]}))))`
  returns an instance that is configured. Mirror
  `byo_from_stored_json_builds_pooled_provider`, or
  `byo_signing_credential_pool_builds_configured_provider` for signing
  vendors. For a new family on an existing vendor, this proves the customer's
  existing key works for the new family.
- **Registry:** `provider_catalog_covers_registry` and
  `model3d_providers_const_matches_the_build_arms` pass, which proves every
  *listed* name builds. They don't catch a build arm missing from its
  `*_PROVIDERS` list, so check the lists by hand.
