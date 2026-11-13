//! OpenAPI aggregator. Drives `/openapi.json` which the first-party SDKs consume.
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "LiteGen API",
        version = env!("CARGO_PKG_VERSION"),
        description = "Universal proxy for AI image, video and 3D model generation."
    ),
    modifiers(&OpenAllOfMembers),
    paths(
        crate::api::handlers::generate_image,
        crate::api::handlers::segment_image,
        crate::api::handlers::estimate_image_cost,
        crate::api::handlers::generate_video,
        crate::api::handlers::estimate_video_cost,
        crate::api::handlers::get_video_status,
        crate::api::handlers::generate_3d,
        crate::api::handlers::estimate_3d_cost,
        crate::api::handlers::get_3d_status,
        crate::api::handlers::convert::convert_model3d,
        crate::api::handlers::convert::list_model3d_convert_formats,
        crate::api::handlers::list_models,
        crate::api::handlers::get_model_schema,
        crate::api::handlers::list_providers,
        crate::api::handlers::health_check,
        crate::api::handlers::liveness,
        crate::api::handlers::get_stats,
        crate::api::handlers::get_logs_filtered,
        crate::api::handlers::create_api_key,
        crate::api::handlers::list_api_keys,
        crate::api::handlers::revoke_api_key,
        crate::api::handlers::get_api_key_handler,
        crate::api::handlers::patch_api_key_handler,
        crate::api::handlers::rotate_api_key,
        crate::api::handlers::test_webhook,
        crate::api::handlers::list_webhook_deliveries,
        crate::api::handlers::clear_cache,
        crate::api::handlers::list_audit,
        // Readiness probe
        crate::api::handlers::readiness,
        // Generations (DB-backed list / get / cancel) + log artifact
        crate::api::handlers::list_generations,
        crate::api::handlers::get_generation,
        crate::api::handlers::list_generation_assets,
        crate::api::handlers::get_generation_asset,
        crate::api::handlers::get_signed_generation_asset,
        crate::api::handlers::revoke_generation_links,
        crate::api::handlers::delete_generation,
        crate::api::handlers::orgs::revoke_app_links,
        crate::api::handlers::management::admin::revoke_links,
        crate::api::handlers::cancel_generation,
        crate::api::handlers::get_log_artifact,
        // Auth — config + password
        crate::api::handlers::auth_password::auth_config,
        crate::api::handlers::auth_password::signup,
        crate::api::handlers::auth_password::login,
        crate::api::handlers::auth_password::logout,
        crate::api::handlers::auth_password::me,
        crate::api::handlers::auth_password::csrf_token,
        crate::api::handlers::auth_password::password_reset_request,
        crate::api::handlers::auth_password::password_reset_confirm,
        // Auth — OAuth
        crate::api::handlers::oauth::github_start,
        crate::api::handlers::oauth::github_callback,
        crate::api::handlers::oauth::google_start,
        crate::api::handlers::oauth::google_callback,
        crate::api::handlers::oauth::oauth_redirect,
        // Users + invitations
        crate::api::handlers::users::list_users,
        crate::api::handlers::users::invite_user,
        crate::api::handlers::users::get_invitation,
        crate::api::handlers::users::accept_invitation,
        crate::api::handlers::users::patch_user,
        crate::api::handlers::users::delete_user,
        crate::api::handlers::users::transfer_owner,
        // Account
        crate::api::handlers::account::get_account,
        crate::api::handlers::account::patch_account,
        crate::api::handlers::account::list_sessions,
        crate::api::handlers::account::revoke_session,
        // Organizations
        crate::api::handlers::orgs::create_org,
        crate::api::handlers::orgs::list_orgs,
        crate::api::handlers::orgs::get_org,
        crate::api::handlers::orgs::patch_org,
        crate::api::handlers::orgs::delete_org,
        // Organization members
        crate::api::handlers::orgs::list_members,
        crate::api::handlers::orgs::invite_member,
        crate::api::handlers::orgs::patch_member,
        crate::api::handlers::orgs::remove_member,
        crate::api::handlers::orgs::transfer_owner,
        // Applications (under orgs + top-level by app id)
        crate::api::handlers::orgs::list_apps,
        crate::api::handlers::orgs::create_app,
        crate::api::handlers::orgs::get_app,
        crate::api::handlers::orgs::patch_app,
        crate::api::handlers::orgs::delete_app,
        // BYO provider credentials
        crate::api::handlers::orgs::list_org_provider_credentials,
        crate::api::handlers::orgs::create_org_provider_credential,
        crate::api::handlers::orgs::delete_org_provider_credential,
        // BYO app storage
        crate::api::handlers::orgs::get_app_storage,
        crate::api::handlers::orgs::put_app_storage,
        crate::api::handlers::orgs::delete_app_storage,
        // Who may open an app's stored files (docs/storage-access.md)
        crate::api::handlers::orgs::get_app_access,
        crate::api::handlers::orgs::put_app_access,
        // Model allowlists (org pool + per-app access)
        crate::api::handlers::orgs::get_org_allowed_models,
        crate::api::handlers::orgs::put_org_allowed_models,
        crate::api::handlers::orgs::get_app_allowed_models,
        crate::api::handlers::orgs::put_app_allowed_models,
        crate::api::handlers::management::limits::list_limits,
        crate::api::handlers::management::limits::my_limits,
        crate::api::handlers::management::limits::get_key_limits,
        crate::api::handlers::management::limits::put_key_limits,
        crate::api::handlers::management::limits::delete_key_limits,
        crate::api::handlers::management::limits::reset_key_limits,
        crate::api::handlers::management::limits::block_key,
        crate::api::handlers::management::limits::unblock_key,
        crate::api::handlers::management::limits::get_org_limits,
        crate::api::handlers::management::limits::put_org_limits,
        crate::api::handlers::management::limits::delete_org_limits,
        crate::api::handlers::management::limits::reset_org_limits,
        crate::api::handlers::management::limits::get_member_limits,
        crate::api::handlers::management::limits::put_member_limits,
        crate::api::handlers::management::limits::delete_member_limits,
        crate::api::handlers::management::limits::reset_member_limits,
        crate::api::handlers::management::limits::get_app_limits,
        crate::api::handlers::management::limits::put_app_limits,
        crate::api::handlers::management::limits::delete_app_limits,
        crate::api::handlers::management::limits::reset_app_limits,
        crate::api::handlers::management::end_users::list_end_users,
        crate::api::handlers::management::end_users::create_end_user,
        crate::api::handlers::management::end_users::get_end_user,
        crate::api::handlers::management::end_users::update_end_user,
        crate::api::handlers::management::end_users::delete_end_user,
        crate::api::handlers::management::end_users::block_end_user,
        crate::api::handlers::management::end_users::unblock_end_user,
        crate::api::handlers::management::end_users::get_end_user_limits,
        crate::api::handlers::management::end_users::put_end_user_limits,
        crate::api::handlers::management::end_users::delete_end_user_limits,
        crate::api::handlers::management::end_users::reset_end_user_limits,
        crate::api::handlers::management::spend::spend_report,
        crate::api::handlers::management::spend::spend_logs,
        crate::api::handlers::management::admin::list_models,
        crate::api::handlers::management::admin::create_model,
        crate::api::handlers::management::admin::get_model,
        crate::api::handlers::management::admin::replace_model,
        crate::api::handlers::management::admin::patch_model,
        crate::api::handlers::management::admin::delete_model,
        crate::api::handlers::management::admin::list_providers,
        crate::api::handlers::management::admin::get_provider,
        crate::api::handlers::management::admin::put_provider,
        crate::api::handlers::management::admin::delete_provider,
        crate::api::handlers::management::admin::get_routes,
        crate::api::handlers::management::admin::put_routes,
        crate::api::handlers::management::admin::delete_routes,
        crate::api::handlers::management::admin::get_settings,
        crate::api::handlers::management::admin::put_settings,
        crate::mcp::oauth::authorize::get_request,
        crate::mcp::oauth::authorize::approve_request,
        crate::mcp::oauth::authorize::deny_request,
        crate::mcp::oauth::grants::list_grants,
        crate::mcp::oauth::grants::revoke_grant,
    ),
    components(schemas(
        // Request / response types (from `types`)
        crate::types::BaseGenerationRequest,
        crate::types::ImageGenerationRequest,
        crate::types::ImageGenerationResponse,
        crate::types::ImageGenerationAccepted,
        crate::types::ImageSegmentationRequest,
        crate::types::ImageSegmentationResponse,
        crate::types::SegmentationPrompt,
        crate::types::SegmentationMask,
        crate::capabilities::SegmentationPromptType,
        crate::capabilities::ParamSpecSegmentationPrompts,
        crate::capabilities::ParamSpecExpand,
        crate::capabilities::ParamSpecLayout,
        crate::capabilities::ParamSpecSourceVideo,
        crate::capabilities::ParamSpecPalette,
        crate::types::PixelSize,
        crate::capabilities::ParamSpecTimeRange,
        crate::capabilities::ExtendOutput,
        crate::types::TimeRange,
        crate::types::VideoSegment,
        crate::types::SegmentOrigin,
        crate::capabilities::LayoutKind,
        crate::types::LayoutItem,
        crate::types::LayoutBox,
        crate::types::Expand,
        crate::types::ImageResult,
        crate::types::VideoGenerationRequest,
        crate::types::VideoGenerationResponse,
        crate::types::Model3dGenerationRequest,
        crate::types::Model3dGenerationResponse,
        crate::types::Model3dAsset,
        crate::types::Model3dAssetKind,
        crate::types::GenerationAsset,
        crate::types::GenerationAssetList,
        // The closed 3D vocabularies. Documentation-only on the request (the
        // fields stay strings so validation owns the error), but named here so
        // both SDKs generate literal types instead of hand-copying them.
        crate::types::Model3dFormat,
        crate::types::Model3dTopology,
        crate::types::Model3dSymmetry,
        crate::types::Model3dSkeleton,
        crate::types::Model3dJointNaming,
        // The canonical presets: a clip's `preset` is a free string (it may be
        // a model's own id), so nothing references this enum — named here so
        // both SDKs carry the vocabulary.
        crate::types::Model3dAnimationPreset,
        crate::types::AnimationRequest,
        crate::types::Region,
        crate::capabilities::MaskConvention,
        crate::capabilities::RegionInput,
        crate::capabilities::PreservesUnmasked,
        crate::capabilities::ModelTask,
        crate::types::ReferenceImage,
        crate::types::RefImageKind,
        crate::types::GenerationStatus,
        crate::types::MediaType,
        crate::types::CostSource,
        crate::types::UsageInfo,
        crate::types::CostEstimate,
        crate::types::ModelInfo,
        crate::types::ModelCapabilities,
        crate::types::ModelPricing,
        crate::types::ProviderConfig,
        crate::types::ApiKeyEntry,
        crate::types::CredentialEntry,
        crate::types::ProviderCatalogEntry,
        crate::types::CredentialFieldSpec,
        crate::types::ModelRoute,
        crate::types::Deployment,
        crate::types::RoutingStrategy,
        crate::types::CacheConfig,
        crate::types::ProviderHealth,
        crate::types::RequestLog,
        crate::types::ProxyStats,
        crate::types::ModelUsageStat,
        crate::types::ProviderUsageStat,
        crate::types::ErrorResponse,
        crate::types::ErrorDetail,
        crate::types::ApiKey,
        crate::types::LivenessResponse,
        crate::types::CacheStatus,
        crate::types::HealthResponse,
        crate::types::ModelListResponse,
        crate::types::ApiKeyInfo,
        crate::types::ApiKeyListResponse,
        crate::types::ApiKeyCreatedResponse,
        crate::types::RevokeKeyResponse,
        crate::types::CacheClearedResponse,
        crate::types::ApiKeyDetail,
        crate::types::UpdateApiKeyRequest,
        crate::types::RotatedKeyResponse,
        crate::types::WebhookDelivery,
        crate::api::handlers::WebhookTestResult,
        // Generations + audit + artifacts + readiness
        crate::types::Generation,
        crate::types::CancelledGeneration,
        crate::api::handlers::CancelGenerationBody,
        crate::types::AuditLogEntry,
        crate::types::RequestArtifact,
        crate::types::ReadinessResponse,
        crate::types::ReadinessChecks,
        // User / session / invitation domain types
        crate::types::User,
        crate::types::Role,
        crate::types::Session,
        crate::types::Invitation,
        crate::types::PasswordReset,
        // Auth handler schemas
        crate::api::handlers::auth_password::AuthConfigResponse,
        crate::api::handlers::auth_password::McpEndpoints,
        crate::api::handlers::auth_password::PublicUser,
        crate::api::handlers::auth_password::AuthResponse,
        crate::api::handlers::auth_password::SignupRequest,
        crate::api::handlers::auth_password::LoginRequest,
        crate::api::handlers::auth_password::CsrfResponse,
        crate::api::handlers::auth_password::PasswordResetRequestBody,
        crate::api::handlers::auth_password::PasswordResetConfirmBody,
        // Users handler schemas
        crate::api::handlers::users::PublicUser,
        crate::api::handlers::users::InviteRequest,
        crate::api::handlers::users::InviteResponse,
        crate::api::handlers::users::InvitationView,
        crate::api::handlers::users::AcceptInvitationRequest,
        crate::api::handlers::users::PatchUserRequest,
        crate::api::handlers::users::TransferOwnerRequest,
        // Account handler schemas
        crate::api::handlers::account::AccountUser,
        crate::api::handlers::account::SessionInfo,
        crate::api::handlers::account::PatchAccountRequest,
        // Tenancy domain types
        crate::types::Organization,
        crate::types::Application,
        crate::types::OrganizationMember,
        crate::types::ProviderCredentialInfo,
        crate::types::AppStorageInfo,
        // Orgs handler schemas (views + request bodies)
        crate::api::handlers::orgs::OrgView,
        crate::api::handlers::orgs::OrgSummary,
        crate::api::handlers::orgs::MemberView,
        crate::api::handlers::orgs::CreateOrgRequest,
        crate::api::handlers::orgs::UpdateOrgRequest,
        crate::api::handlers::orgs::CreateAppRequest,
        crate::api::handlers::orgs::UpdateAppRequest,
        crate::api::handlers::orgs::AddMemberRequest,
        crate::api::handlers::orgs::UpdateMemberRequest,
        crate::api::handlers::orgs::TransferOwnerRequest,
        crate::api::handlers::orgs::CreateProviderCredentialRequest,
        crate::api::handlers::orgs::PutAppStorageRequest,
        // Stored-file access (docs/storage-access.md)
        crate::access::AccessMode,
        crate::access::OwnerScope,
        crate::access::AccessSettings,
        crate::access::AppAccessOverrides,
        crate::access::PutAppAccessRequest,
        crate::access::AppAccessSettings,
        crate::access::LinksRevoked,
        crate::access::SignedLinks,
        // Model allowlists (org pool + per-app access)
        crate::types::OrgAllowedModels,
        crate::types::AppModelAccess,
        crate::types::SetOrgAllowedModelsRequest,
        crate::types::SetAppModelAccessRequest,
        // Capability schema (response of GET /v1/models/{id}).
        // `MediaType` and `ModelPricing` are renamed via `#[schema(as = ...)]` to
        // avoid collisions with the same-named types in `crate::types`.
        crate::capabilities::ModelSchema,
        crate::capabilities::ModelCapabilityFlags,
        crate::capabilities::ModelPricing,
        crate::capabilities::PromptSpec,
        crate::capabilities::ParamSpec,
        crate::capabilities::ParamSpecBool,
        crate::capabilities::ParamSpecInt,
        crate::capabilities::ParamSpecFloat,
        crate::capabilities::ParamSpecString,
        crate::capabilities::ParamSpecStringArray,
        crate::capabilities::ParamSpecAspectRatio,
        crate::capabilities::ParamSpecSeed,
        crate::capabilities::SizeSpec,
        crate::capabilities::SizeSpecFreeform,
        crate::capabilities::SizeSpecEnum,
        crate::capabilities::RefInputSpec,
        crate::capabilities::RefRoleSpec,
        crate::capabilities::RefProviderFormat,
        crate::capabilities::RefProviderFormatMultipart,
        crate::capabilities::MediaType,
        crate::management::types::EntityType,
        crate::management::types::EntityLimits,
        crate::management::types::LimitsInput,
        crate::management::types::LimitsListResponse,
        crate::management::types::EndUser,
        crate::management::types::EndUserView,
        crate::management::types::CreateEndUserRequest,
        crate::management::types::UpdateEndUserRequest,
        crate::management::types::EndUserListResponse,
        crate::management::types::SpendGroupBy,
        crate::management::types::SpendReportRow,
        crate::management::types::SpendReportResponse,
        crate::management::types::SpendLogEntry,
        crate::management::types::SpendLogsResponse,
        crate::management::types::ModelSource,
        crate::management::types::AdminModelView,
        crate::management::types::AdminModelListResponse,
        crate::management::types::PatchModelStateRequest,
        crate::management::types::ProviderSettings,
        crate::management::types::PutProviderConfigRequest,
        crate::management::types::ProviderConfigView,
        crate::management::types::ProviderConfigListResponse,
        crate::management::types::RouteDeployment,
        crate::management::types::RouteSpec,
        crate::management::types::RoutesDocument,
        crate::management::types::GatewaySettings,
        crate::management::types::GatewaySettingsView,
        crate::api::handlers::management::DeletedResponse,
        crate::api::handlers::management::limits::KeyActiveResponse,
        crate::mcp::oauth::authorize::OAuthRequestView,
        crate::mcp::oauth::authorize::OAuthClientView,
        crate::mcp::oauth::authorize::OAuthOrgChoice,
        crate::mcp::oauth::authorize::OAuthAppChoice,
        crate::mcp::oauth::authorize::ApproveOAuthRequest,
        crate::mcp::oauth::authorize::OAuthRedirect,
        crate::mcp::oauth::grants::ConnectedApp,
        crate::mcp::oauth::grants::RevokedGrant,
    )),
    tags(
        (name = "Images", description = "Image generation endpoints"),
        (name = "Videos", description = "Video generation endpoints"),
        (name = "Models3D", description = "3D (mesh) generation endpoints"),
        (name = "Utilities", description = "Standalone tools that call no provider, such as mesh format conversion"),
        (name = "Models", description = "Model discovery"),
        (name = "Providers", description = "Supported provider catalog"),
        (name = "System", description = "Health checks"),
        (name = "Dashboard", description = "Stats, logs, and artifacts"),
        (name = "Generations", description = "DB-backed generation list, status, and cancel"),
        (name = "Admin", description = "API key management, webhooks, and audit log"),
        (name = "Auth", description = "Authentication (password + OAuth)"),
        (name = "Users", description = "User management and invitations"),
        (name = "Account", description = "Current user account and sessions"),
        (name = "Organizations", description = "Organization CRUD, members, and ownership"),
        (name = "Applications", description = "Applications and BYO provider credentials"),
        (name = "Budgets", description = "Budgets, budget windows, model allowlists and rate/parallel limits for keys, orgs, apps and members"),
        (name = "End Users", description = "End users (your callers' customers): tracking, blocking and per-end-user budgets"),
        (name = "Spend", description = "Spend reports and the attributed request log"),
        (name = "Gateway", description = "Gateway admin: runtime models, provider configs, routing and settings"),
        (name = "OAuth", description = "Sign-in for MCP clients: the consent page's requests and the apps a person has connected"),
    )
)]
pub struct ApiDoc;

/// D2: the catalog structs deny unknown fields, which utoipa draws as
/// `additionalProperties: false`. Right for a plain struct; but a tagged
/// payload (a `ParamSpec*` beside its `kind`) is drawn as a `$ref` member of
/// an `allOf`, and closed it would refuse its own tag, so no payload could
/// pass a strict validator. Every component used that way stays open here;
/// the server still refuses an unknown key when it reads one.
struct OpenAllOfMembers;

impl utoipa::Modify for OpenAllOfMembers {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::schema::{AdditionalProperties, Schema};
        fn members(v: &serde_json::Value, out: &mut std::collections::BTreeSet<String>) {
            match v {
                serde_json::Value::Object(m) => {
                    for part in m.get("allOf").and_then(serde_json::Value::as_array).into_iter().flatten() {
                        if let Some(r) = part.get("$ref").and_then(serde_json::Value::as_str) {
                            out.insert(r.trim_start_matches("#/components/schemas/").to_string());
                        }
                    }
                    m.values().for_each(|c| members(c, out));
                }
                serde_json::Value::Array(a) => a.iter().for_each(|c| members(c, out)),
                _ => {}
            }
        }
        let mut names = std::collections::BTreeSet::new();
        members(&serde_json::to_value(&*openapi).unwrap_or_default(), &mut names);
        let Some(components) = openapi.components.as_mut() else { return };
        for name in names {
            if let Some(utoipa::openapi::RefOr::T(Schema::Object(o))) = components.schemas.get_mut(&name) {
                if matches!(o.additional_properties.as_deref(), Some(AdditionalProperties::FreeForm(false))) {
                    o.additional_properties = None;
                }
            }
        }
    }
}

/// A key a request body sent that its published schema documents no property
/// for (E2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnknownField {
    /// Where it sits: `colour`, `reference_images[0].weight`.
    pub path: String,
    /// Sent at the top level: `path` is the key itself.
    pub top_level: bool,
}

/// The request schemas whose fields the validator knows (E2).
pub(crate) const REQUEST_SCHEMAS: [&str; 4] =
    ["ImageGenerationRequest", "VideoGenerationRequest", "Model3dGenerationRequest", "ImageSegmentationRequest"];

/// The published document, built once: what `/openapi.json` serves and the
/// MCP tools are made from. A field is known when its request schema here
/// documents it. utoipa derives these schemas from the request types' own
/// serde attributes (rename, flatten, tag), and the types use no serde
/// `alias` or custom deserializer, so a documented name is exactly a name
/// serde reads.
fn document() -> &'static serde_json::Value {
    static DOC: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    DOC.get_or_init(|| serde_json::to_value(ApiDoc::openapi()).expect("the OpenAPI document serialises"))
}

/// The top-level property names a request schema documents (E2): one of
/// [`REQUEST_SCHEMAS`]. Panics on any other name.
pub(crate) fn request_fields(schema: &str) -> &'static std::collections::BTreeSet<String> {
    use std::collections::BTreeMap;
    static FIELDS: std::sync::OnceLock<BTreeMap<&'static str, std::collections::BTreeSet<String>>> = std::sync::OnceLock::new();
    let all = FIELDS.get_or_init(|| {
        let doc = document();
        REQUEST_SCHEMAS
            .iter()
            .map(|name| match keys(doc, &doc["components"]["schemas"][*name], None, 0) {
                Keys::Closed(props) => (*name, props.keys().map(|k| k.to_string()).collect()),
                _ => panic!("the published {name} is no object with fixed properties"),
            })
            .collect()
    });
    all.get(schema).unwrap_or_else(|| panic!("{schema} is not one of the request schemas {REQUEST_SCHEMAS:?}"))
}

/// Every key of `body` that the request schema `schema` (one of
/// [`REQUEST_SCHEMAS`]) documents no property for, at any depth, depth first
/// in key order (E2, N6). A key sent as `null` asks for nothing and is not
/// counted, and free-form objects (`extra`, `metadata`) take any key.
pub(crate) fn unknown_fields(schema: &str, body: &serde_json::Value) -> Vec<UnknownField> {
    request_fields(schema); // the same names, and the same panic on a name that is no request
    let doc = document();
    let mut out = Vec::new();
    walk(doc, &doc["components"]["schemas"][schema], body, "", &mut out);
    out
}

/// What a schema allows for an object's keys.
#[derive(Clone)]
enum Keys<'a> {
    /// These properties and no others, each with its schema.
    Closed(std::collections::BTreeMap<&'a str, &'a serde_json::Value>),
    /// Any key: `additionalProperties`, or a schema that says nothing.
    Open,
    /// No object: null, a scalar or an array.
    NotObject,
}

/// Past this many `$ref`, `allOf` or `oneOf` steps a schema counts as open,
/// so a cycle can't loop.
const MAX_SCHEMA_DEPTH: u8 = 32;

fn deref<'a>(doc: &'a serde_json::Value, mut s: &'a serde_json::Value) -> &'a serde_json::Value {
    static NULL: serde_json::Value = serde_json::Value::Null;
    for _ in 0..MAX_SCHEMA_DEPTH {
        match s.get("$ref").and_then(serde_json::Value::as_str).and_then(|r| r.strip_prefix('#')) {
            Some(pointer) => s = doc.pointer(pointer).unwrap_or(&NULL),
            None => break,
        }
    }
    s
}

/// The keys schema `s` allows on an object. `body`, when given, picks the
/// variant of a tagged `oneOf` (`SegmentationPrompt`) by its tag, as serde
/// does; without one, or when no single variant matches, any variant's key
/// counts.
fn keys<'a>(doc: &'a serde_json::Value, s: &'a serde_json::Value, body: Option<&serde_json::Map<String, serde_json::Value>>, depth: u8) -> Keys<'a> {
    use serde_json::Value;
    if depth >= MAX_SCHEMA_DEPTH {
        return Keys::Open;
    }
    let s = deref(doc, s);
    if let Some(parts) = s.get("allOf").and_then(Value::as_array) {
        return union(parts.iter().map(|p| keys(doc, p, body, depth + 1)).collect());
    }
    if let Some(variants) = s.get("oneOf").or_else(|| s.get("anyOf")).and_then(Value::as_array) {
        let views: Vec<Keys> = variants.iter().map(|v| keys(doc, v, body, depth + 1)).collect();
        if let Some(body) = body {
            let tagged: Vec<&Keys> = views
                .iter()
                .filter(|k| matches!(k, Keys::Closed(props) if props.iter().any(|(name, prop)| tag_of(doc, prop).is_some_and(|t| body.get(*name) == Some(t)))))
                .collect();
            if let [only] = tagged.as_slice() {
                return (*only).clone();
            }
        }
        return union(views);
    }
    if s.get("properties").is_some() || type_is(s, "object") {
        if s.get("additionalProperties").is_some_and(|a| a != &Value::Bool(false)) {
            return Keys::Open;
        }
        let props = s.get("properties").and_then(Value::as_object);
        return Keys::Closed(props.map(|p| p.iter().map(|(k, v)| (k.as_str(), v)).collect()).unwrap_or_default());
    }
    // A schema that names no type, items, enum or const takes any value (`extra`).
    if ["type", "items", "enum", "const"].iter().all(|k| s.get(k).is_none()) {
        return Keys::Open;
    }
    Keys::NotObject
}

/// Keys allowed by all of these parts (`allOf`) or any of these variants
/// (`oneOf`): their properties together; open when one of them is.
fn union(views: Vec<Keys<'_>>) -> Keys<'_> {
    let mut out: Option<std::collections::BTreeMap<&str, &serde_json::Value>> = None;
    for view in views {
        match view {
            Keys::Open => return Keys::Open,
            Keys::Closed(props) => out.get_or_insert_with(Default::default).extend(props),
            Keys::NotObject => {}
        }
    }
    out.map_or(Keys::NotObject, Keys::Closed)
}

/// A property that takes exactly one value (a tagged variant's `type`).
fn tag_of<'a>(doc: &'a serde_json::Value, prop: &'a serde_json::Value) -> Option<&'a serde_json::Value> {
    let prop = deref(doc, prop);
    match prop.get("enum").and_then(serde_json::Value::as_array).map(Vec::as_slice) {
        Some([only]) => Some(only),
        _ => prop.get("const"),
    }
}

fn type_is(s: &serde_json::Value, wanted: &str) -> bool {
    match s.get("type") {
        Some(serde_json::Value::String(t)) => t == wanted,
        Some(serde_json::Value::Array(ts)) => ts.iter().any(|t| t == wanted),
        _ => false,
    }
}

/// The schema of an array's items, through `$ref`, `allOf` and `oneOf`.
fn items<'a>(doc: &'a serde_json::Value, s: &'a serde_json::Value, depth: u8) -> Option<&'a serde_json::Value> {
    if depth >= MAX_SCHEMA_DEPTH {
        return None;
    }
    let s = deref(doc, s);
    if let Some(item) = s.get("items") {
        return Some(item);
    }
    ["allOf", "oneOf", "anyOf"]
        .iter()
        .filter_map(|k| s.get(*k).and_then(serde_json::Value::as_array))
        .flatten()
        .find_map(|part| items(doc, part, depth + 1))
}

fn walk(doc: &serde_json::Value, schema: &serde_json::Value, body: &serde_json::Value, path: &str, out: &mut Vec<UnknownField>) {
    match body {
        serde_json::Value::Object(map) => {
            let Keys::Closed(props) = keys(doc, schema, Some(map), 0) else { return };
            let mut names: Vec<&String> = map.keys().collect();
            names.sort();
            for name in names {
                let value = &map[name];
                if value.is_null() {
                    continue;
                }
                let at = if path.is_empty() { name.clone() } else { format!("{path}.{name}") };
                match props.get(name.as_str()) {
                    Some(child) => walk(doc, child, value, &at, out),
                    None => out.push(UnknownField { path: at, top_level: path.is_empty() }),
                }
            }
        }
        serde_json::Value::Array(list) => {
            if let Some(item) = items(doc, schema, 0) {
                for (i, value) in list.iter().enumerate() {
                    walk(doc, item, value, &format!("{path}[{i}]"), out);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    //! What the generated document promises the SDK generators. Both SDKs are
    //! regenerated from this output, so a shape that silently changes here
    //! changes every client's types.
    use super::ApiDoc;
    use serde_json::{json, Value};
    use utoipa::OpenApi;

    fn doc() -> Value {
        serde_json::to_value(ApiDoc::openapi()).expect("openapi serialises")
    }

    /// The object carrying `properties[name]` for a schema, looking through the
    /// `allOf` a `#[serde(flatten)]` produces.
    fn property<'a>(schema: &'a Value, name: &str) -> &'a Value {
        if let Some(p) = schema["properties"].get(name) {
            return p;
        }
        schema["allOf"]
            .as_array()
            .and_then(|parts| parts.iter().find_map(|p| p["properties"].get(name)))
            .unwrap_or_else(|| panic!("no property '{name}' in {schema}"))
    }

    fn mentions(v: &Value, needle: &str) -> bool {
        v.to_string().contains(needle)
    }

    #[test]
    fn the_3d_vocabularies_are_named_enum_schemas() {
        let d = doc();
        let s = &d["components"]["schemas"];
        assert_eq!(
            s["Model3dFormat"]["enum"],
            json!(["glb", "gltf", "obj", "stl", "ply", "fbx", "usdz", "usda", "usdc", "3mf", "dae"]),
        );
        assert_eq!(s["Model3dTopology"]["enum"], json!(["triangle", "quad"]));
        assert_eq!(s["Model3dSymmetry"]["enum"], json!(["off", "auto", "on"]));
        assert_eq!(
            s["Model3dSkeleton"]["enum"],
            json!(["humanoid", "quadruped", "hexapod", "octopod", "avian", "serpentine", "aquatic", "other"]),
        );
        assert_eq!(s["Model3dJointNaming"]["enum"], json!(["vendor", "vrm", "mixamo", "unity", "unreal"]));
    }

    #[test]
    fn the_3d_request_types_its_clips_and_the_catalog_types_their_spec() {
        let d = doc();
        let s = &d["components"]["schemas"];
        let req = &s["Model3dGenerationRequest"];
        assert!(mentions(property(req, "animations"), "#/components/schemas/AnimationRequest"), "{}", property(req, "animations"));
        assert!(mentions(property(req, "fps"), "integer"), "{}", property(req, "fps"));
        for field in ["name", "preset", "prompt", "duration_s", "in_place", "loop"] {
            property(&s["AnimationRequest"], field);
        }
        assert_eq!(s["AnimationRequest"]["required"], json!(["name"]));
        assert_eq!(
            s["Model3dAnimationPreset"]["enum"],
            json!(["idle", "walk", "run", "jump", "fall", "attack_melee", "attack_ranged", "cast", "hurt", "death", "crouch", "climb", "turn"]),
        );
        for field in ["max_items", "presets", "text_to_motion", "in_place"] {
            property(&s["ParamSpecAnimationList"], field);
        }
        assert_eq!(s["InPlaceMode"]["enum"], json!(["litegen", "native"]));
        assert!(mentions(&s["ParamSpec"], "animation_list"), "the union names the new kind");
    }

    #[test]
    fn an_image_edit_documents_its_region_and_the_catalog_its_conventions() {
        let d = doc();
        let s = &d["components"]["schemas"];
        let req = &s["ImageGenerationRequest"];
        assert!(mentions(property(req, "region"), "#/components/schemas/Region"), "{}", property(req, "region"));
        assert!(mentions(property(req, "mask_grow_px"), "integer"), "{}", property(req, "mask_grow_px"));
        for f in ["x", "y", "w", "h"] {
            property(&s["Region"], f);
        }
        assert_eq!(s["MaskConvention"]["enum"], json!(["white_edits", "black_edits", "transparent_edits"]));
        assert_eq!(s["RegionInput"]["enum"], json!(["box", "mask"]));
        assert_eq!(s["PreservesUnmasked"]["enum"], json!(["exact", "similar", "unspecified"]));
        for f in ["supports_region_edit", "region_input", "preserves_unmasked", "mask_convention"] {
            property(&s["ModelCapabilities"], f);
        }
        assert!(mentions(&s["ParamSpec"], "\"region\""), "the union names the region kind");
    }

    #[test]
    fn pixel_art_documents_the_palette_directions_and_catalog_fields() {
        let d = doc();
        let s = &d["components"]["schemas"];
        let req = &s["ImageGenerationRequest"];
        assert!(mentions(property(req, "palette"), "string"));
        assert!(mentions(property(req, "directions"), "integer"));
        assert!(mentions(property(&s["ImageResult"], "direction"), "string"));
        for f in ["native_pixel_art", "pixel_size", "supports_palette", "supports_forced_palette", "directions"] {
            property(&s["ModelCapabilities"], f);
        }
        for f in ["min", "max"] {
            property(&s["PixelSize"], f);
        }
        for f in ["forced", "max_colors"] {
            property(&s["ParamSpecPalette"], f);
        }
        assert!(mentions(property(&s["ModelCapabilityFlags"], "native_pixel_art"), "boolean"));
    }

    #[test]
    fn video_operations_document_their_source_window_and_segments() {
        let d = doc();
        let s = &d["components"]["schemas"];
        let req = &s["VideoGenerationRequest"];
        assert!(mentions(property(req, "source_video"), "#/components/schemas/SourceRef"));
        assert!(mentions(property(req, "time_range"), "#/components/schemas/TimeRange"));
        for f in ["direction", "replace"] {
            property(req, f);
        }
        assert!(mentions(property(&s["VideoGenerationResponse"], "segments"), "#/components/schemas/VideoSegment"));
        for f in ["start_s", "end_s", "origin"] {
            property(&s["VideoSegment"], f);
        }
        assert_eq!(s["SegmentOrigin"]["enum"], json!(["source", "generated"]));
        assert_eq!(s["ExtendOutput"]["enum"], json!(["full", "added"]));
        for f in ["supports_source_video", "extend_output", "supports_time_range"] {
            property(&s["ModelCapabilities"], f);
        }
        for f in ["formats", "max_duration_s", "own_generations_only", "extend_output"] {
            property(&s["ParamSpecSourceVideo"], f);
        }
        for f in ["min_window_s", "whole_seconds"] {
            property(&s["ParamSpecTimeRange"], f);
        }
    }

    #[test]
    fn preservation_documents_its_two_fields_and_the_catalog_flag() {
        let d = doc();
        let s = &d["components"]["schemas"];
        let req = &s["ImageGenerationRequest"];
        assert!(mentions(property(req, "preserve_unmasked"), "boolean"));
        assert!(mentions(property(req, "feather_px"), "integer"));
        assert!(mentions(property(&s["ModelCapabilities"], "supports_preserve_unmasked"), "boolean"));
    }

    #[test]
    fn layout_and_mask_prompt_document_their_boxes_and_the_catalog_flags() {
        let d = doc();
        let s = &d["components"]["schemas"];
        let req = &s["ImageGenerationRequest"];
        assert!(mentions(property(req, "layout"), "#/components/schemas/LayoutItem"));
        assert!(mentions(property(req, "mask_prompt"), "string"));
        for f in ["box", "kind", "prompt", "text"] {
            property(&s["LayoutItem"], f);
        }
        for f in ["x", "y", "w", "h"] {
            assert!(mentions(property(&s["LayoutBox"], f), "number"), "{f}");
        }
        assert_eq!(s["LayoutKind"]["enum"], json!(["object", "text"]));
        for f in ["supports_layout", "layout_kinds", "supports_mask_prompt"] {
            property(&s["ModelCapabilities"], f);
        }
        for f in ["kinds", "max_items", "single_word_text"] {
            property(&s["ParamSpecLayout"], f);
        }
    }

    #[test]
    fn expand_documents_its_sides_the_delivered_size_and_the_catalog_spec() {
        let d = doc();
        let s = &d["components"]["schemas"];
        assert!(mentions(property(&s["ImageGenerationRequest"], "expand"), "#/components/schemas/Expand"));
        for f in ["left", "top", "right", "bottom"] {
            assert!(mentions(property(&s["Expand"], f), "integer"), "{f}");
        }
        for f in ["width", "height"] {
            assert!(mentions(property(&s["ImageResult"], f), "integer"), "{f}");
        }
        assert!(mentions(property(&s["ModelCapabilities"], "supports_expand"), "boolean"));
        for f in ["centered_only", "max_px"] {
            property(&s["ParamSpecExpand"], f);
        }
        assert!(mentions(&s["ParamSpec"], "\"expand\""), "the union names the new kind");
    }

    #[test]
    fn placement_is_three_request_fields_and_a_listed_capability() {
        let d = doc();
        let s = &d["components"]["schemas"];
        let req = &s["Model3dGenerationRequest"];
        for f in ["ground", "center"] {
            assert!(mentions(property(req, f), "boolean"), "{f}: {}", property(req, f));
        }
        assert!(mentions(property(req, "height_m"), "number"));
        assert!(mentions(property(&s["ModelCapabilities"], "supports_placement"), "boolean"));
    }

    #[test]
    fn packages_are_a_request_field_an_asset_kind_and_a_named_refusal() {
        let d = doc();
        let s = &d["components"]["schemas"];
        let req = &s["Model3dGenerationRequest"];
        assert!(mentions(property(req, "output_packages"), "boolean"), "{}", property(req, "output_packages"));
        assert!(mentions(&s["Model3dAssetKind"], "\"package\""), "{}", s["Model3dAssetKind"]);
        let refusals = d["paths"]["/v1/models3d/generations"]["post"]["responses"]["400"]["description"].as_str().unwrap_or("");
        assert!(refusals.contains("option_needs_keeping"), "{refusals}");
    }

    #[test]
    fn texture_remesh_and_segment_document_their_params_and_the_parts() {
        let d = doc();
        let s = &d["components"]["schemas"];
        let req = &s["Model3dGenerationRequest"];
        assert!(mentions(property(req, "texture_resolution"), "integer"), "{}", property(req, "texture_resolution"));
        assert!(mentions(property(req, "keep_uvs"), "boolean"), "{}", property(req, "keep_uvs"));
        assert!(mentions(property(&s["SceneManifest"], "parts"), "#/components/schemas/ScenePart"), "{}", property(&s["SceneManifest"], "parts"));
        for f in ["name", "node", "triangle_count", "bounds"] {
            property(&s["ScenePart"], f);
        }
        assert_eq!(s["ScenePart"]["required"], json!(["name", "node", "triangle_count"]));
        assert!(mentions(property(&s["ModelCapabilities"], "texture_resolutions"), "integer"));
        assert!(mentions(property(&s["ModelCapabilities"], "supports_keep_uvs"), "boolean"));
    }

    #[test]
    fn background_removal_is_a_catalog_flag_and_a_listed_capability() {
        let d = doc();
        let s = &d["components"]["schemas"];
        assert!(mentions(property(&s["ModelCapabilityFlags"], "background_removal"), "boolean"));
        assert!(mentions(property(&s["ModelCapabilities"], "supports_background_removal"), "boolean"));
    }

    #[test]
    fn segmentation_documents_its_prompts_its_masks_and_the_catalog_fields() {
        let d = doc();
        let op = &d["paths"]["/v1/images/segmentations"]["post"];
        assert!(mentions(&op["requestBody"], "#/components/schemas/ImageSegmentationRequest"), "{op}");
        assert!(mentions(&op["responses"]["200"], "#/components/schemas/ImageSegmentationResponse"), "{op}");
        for code in ["prompt_out_of_bounds", "image_unreadable", "model_task_mismatch", "param_conflict"] {
            assert!(mentions(&op["responses"]["400"], code), "{code}");
        }
        let s = &d["components"]["schemas"];
        assert_eq!(s["ImageSegmentationRequest"]["required"], json!(["model", "image", "prompts"]));
        assert!(mentions(property(&s["ImageSegmentationRequest"], "prompts"), "#/components/schemas/SegmentationPrompt"));
        for kind in ["\"point\"", "\"box\"", "\"text\"", "label"] {
            assert!(mentions(&s["SegmentationPrompt"], kind), "{kind}: {}", s["SegmentationPrompt"]);
        }
        for f in ["png_base64", "score", "box"] {
            property(&s["SegmentationMask"], f);
        }
        // Always present; a score is null when the vendor gives none.
        assert_eq!(s["SegmentationMask"]["required"], json!(["png_base64", "score", "box"]));
        assert!(mentions(property(&s["ImageSegmentationResponse"], "masks"), "#/components/schemas/SegmentationMask"));
        assert_eq!(s["SegmentationPromptType"]["enum"], json!(["point", "box", "text"]));
        for f in ["supports_segmentation", "segmentation_prompts"] {
            property(&s["ModelCapabilities"], f);
        }
        for f in ["types", "max_items", "max_masks"] {
            property(&s["ParamSpecSegmentationPrompts"], f);
        }
        assert!(mentions(&s["ParamSpec"], "segmentation_prompts"), "the union names the new kind");
    }

    #[test]
    fn multiview_documents_its_views_and_the_view_each_image_shows() {
        let d = doc();
        let s = &d["components"]["schemas"];
        assert_eq!(
            s["ImageView"]["enum"],
            json!(["front", "front-left", "left", "back-left", "back", "back-right", "right", "front-right", "top", "bottom"]),
        );
        let view = "#/components/schemas/ImageView";
        assert!(mentions(property(&s["ImageGenerationRequest"], "views"), view), "{}", property(&s["ImageGenerationRequest"], "views"));
        assert!(mentions(property(&s["ImageResult"], "view"), view), "{}", property(&s["ImageResult"], "view"));
        assert!(mentions(property(&s["ImageResult"], "yaw_deg"), "number"), "{}", property(&s["ImageResult"], "yaw_deg"));
        assert!(mentions(property(&s["RefInputSpec"], "view_order"), view), "{}", property(&s["RefInputSpec"], "view_order"));
    }

    #[test]
    fn timed_keyframes_document_their_time_and_each_models_timing() {
        let d = doc();
        let s = &d["components"]["schemas"];
        assert!(mentions(property(&s["ReferenceImage"], "time_s"), "number"), "{}", property(&s["ReferenceImage"], "time_s"));
        assert_eq!(s["KeyframeTiming"]["enum"], json!(["any", "even"]));
        let caps = &s["ModelCapabilities"];
        assert!(mentions(property(caps, "supports_keyframes"), "boolean"));
        assert!(mentions(property(caps, "max_keyframes"), "integer"));
        assert!(mentions(property(caps, "keyframe_timing"), "#/components/schemas/KeyframeTiming"), "{}", property(caps, "keyframe_timing"));
        for f in ["timing", "time_step_s"] {
            property(&s["RefRoleSpec"], f);
        }
    }

    /// D2: the catalog's schemas say what the server enforces, a key they do
    /// not name is refused (`POST /v1/admin/models`), and the served
    /// `status` is documented.
    #[test]
    fn the_catalog_schemas_refuse_unknown_fields() {
        let d = doc();
        let s = &d["components"]["schemas"];
        for name in [
            "ModelSchema", "CapabilityModelPricing", "ModelCapabilityFlags", "PromptSpec", "RefInputSpec", "RefRoleSpec",
            "NativeOrientation", "AnimationPresetSpec", "TextToMotionSpec",
        ] {
            assert_eq!(s[name]["additionalProperties"], json!(false), "{name}: {}", s[name]);
        }
        assert!(mentions(property(&s["ModelSchema"], "status"), "#/components/schemas/ModelStatus"), "{}", s["ModelSchema"]);
    }

    /// E4b: a model's extras carry their types, and the request's `extra`
    /// says where to find them.
    #[test]
    fn the_catalog_types_extras_and_the_request_points_to_them() {
        let d = doc();
        let s = &d["components"]["schemas"];
        property(&s["ModelSchema"], "extra_params");
        let extra = property(&s["BaseGenerationRequest"], "extra");
        for needle in ["extra_allowlist", "extra_params", "webhook"] {
            assert!(mentions(&extra["description"], needle), "{needle}: {extra}");
        }
    }

    /// D2: a schema drawn as a `$ref` member of an `allOf` (a tagged
    /// `ParamSpec` payload beside its `kind`) stays open, or a strict
    /// validator would refuse the tag against it and no payload could pass.
    #[test]
    fn a_schema_used_inside_an_allof_stays_open_to_its_siblings() {
        fn refs_in_all_of(v: &Value, out: &mut Vec<String>) {
            match v {
                Value::Object(m) => {
                    for part in m.get("allOf").and_then(Value::as_array).into_iter().flatten() {
                        if let Some(r) = part["$ref"].as_str() {
                            out.push(r.trim_start_matches("#/components/schemas/").to_string());
                        }
                    }
                    m.values().for_each(|c| refs_in_all_of(c, out));
                }
                Value::Array(a) => a.iter().for_each(|c| refs_in_all_of(c, out)),
                _ => {}
            }
        }
        let d = doc();
        let mut members = Vec::new();
        refs_in_all_of(&d, &mut members);
        assert!(members.iter().any(|m| m == "ParamSpecRegion"), "{members:?}");
        for name in members {
            let schema = &d["components"]["schemas"][&name];
            assert_ne!(schema["additionalProperties"], json!(false), "{name} is closed: {schema}");
        }
    }

    #[test]
    fn a_models_price_documents_the_rules_that_follow_the_request() {
        let d = doc();
        let s = &d["components"]["schemas"];
        for pricing in ["ModelPricing", "CapabilityModelPricing"] {
            let rules = property(&s[pricing], "variable_pricing");
            assert!(mentions(rules, "#/components/schemas/PriceRule"), "{pricing}: {rules}");
        }
        for f in ["when", "usd", "per_second_usd", "per_megapixel_usd", "per_item_usd", "per_item", "multiply", "add_usd"] {
            property(&s["PriceRule"], f);
        }
    }

    #[test]
    fn a_freeform_size_documents_the_vendors_pixel_and_aspect_limits() {
        let d = doc();
        for f in ["min_pixels", "max_pixels", "max_aspect"] {
            property(&d["components"]["schemas"]["SizeSpecFreeform"], f);
        }
    }

    #[test]
    fn the_listing_documents_lifecycle_limits_and_its_version() {
        let d = doc();
        let s = &d["components"]["schemas"];
        assert_eq!(s["ModelStatus"]["enum"], json!(["active", "deprecated", "retired"]));
        assert!(mentions(property(&s["ModelInfo"], "status"), "#/components/schemas/ModelStatus"));
        for f in ["deprecated_at", "retires_at", "replaced_by"] {
            property(&s["ModelInfo"], f);
            property(&s["ModelSchema"], f);
        }
        property(&s["ModelCapabilities"], "max_reference_images");
        assert!(mentions(property(&s["ModelListResponse"], "catalog_version"), "string"));
        assert!(s["ModelListResponse"]["required"].as_array().unwrap().iter().any(|r| r == "catalog_version"));
    }

    #[test]
    fn an_error_documents_the_vendors_own_answer_and_whether_to_retry() {
        let d = doc();
        let s = &d["components"]["schemas"];
        let detail = &s["ErrorDetail"];
        assert!(mentions(property(detail, "code"), "string"), "{}", property(detail, "code"));
        assert!(mentions(property(detail, "retryable"), "boolean"), "{}", property(detail, "retryable"));
        assert!(mentions(property(detail, "provider_error"), "#/components/schemas/VendorError"), "{}", property(detail, "provider_error"));
        for f in ["status", "code", "message"] {
            property(&s["VendorError"], f);
        }
    }

    #[test]
    fn an_image_request_documents_its_container_and_background() {
        let d = doc();
        let s = &d["components"]["schemas"];
        assert_eq!(s["ImageOutputFormat"]["enum"], json!(["png", "webp", "jpeg"]));
        assert_eq!(s["ImageBackground"]["enum"], json!(["transparent", "opaque"]));
        let req = &s["ImageGenerationRequest"];
        assert!(mentions(property(req, "output_format"), "#/components/schemas/ImageOutputFormat"), "{}", property(req, "output_format"));
        assert!(mentions(property(req, "background"), "#/components/schemas/ImageBackground"), "{}", property(req, "background"));
        for f in ["supports_transparent_background", "lossy_only", "output_formats"] {
            property(&s["ModelCapabilities"], f);
        }
    }

    #[test]
    fn a_video_request_documents_its_sprite_controls_and_the_result_its_media() {
        let d = doc();
        let s = &d["components"]["schemas"];
        assert_eq!(s["VideoCamera"]["enum"], json!(["fixed", "auto"]));
        let req = &s["VideoGenerationRequest"];
        assert!(mentions(property(req, "camera"), "#/components/schemas/VideoCamera"), "{}", property(req, "camera"));
        assert!(mentions(property(req, "audio"), "boolean"));
        assert!(mentions(property(req, "loop"), "boolean"));
        for f in ["supports_fixed_camera", "supports_audio_toggle", "supports_loop", "duration_values"] {
            property(&s["ModelCapabilities"], f);
        }
        assert!(mentions(property(&s["VideoGenerationResponse"], "media"), "#/components/schemas/VideoMedia"));
        for f in ["duration_s", "fps", "width", "height", "has_audio"] {
            property(&s["VideoMedia"], f);
        }
        property(&s["ParamSpecFloat"], "values");
    }

    #[test]
    fn a_models_reference_spec_says_whether_its_vendor_takes_data_uris() {
        let d = doc();
        let p = property(&d["components"]["schemas"]["RefInputSpec"], "data_uri");
        assert_eq!(p["type"], "boolean", "{p}");
    }

    #[test]
    fn the_3d_request_types_the_rig_params() {
        let d = doc();
        let req = &d["components"]["schemas"]["Model3dGenerationRequest"];
        assert!(mentions(property(req, "skeleton"), "#/components/schemas/Model3dSkeleton"));
        assert!(mentions(property(req, "joint_naming"), "#/components/schemas/Model3dJointNaming"));
        assert!(mentions(property(req, "height_m"), "\"number\""), "{}", property(req, "height_m"));
    }

    #[test]
    fn the_3d_request_types_its_vocabulary_and_deprecates_the_scalar() {
        let d = doc();
        let req = &d["components"]["schemas"]["Model3dGenerationRequest"];
        let formats = property(req, "output_formats");
        assert!(mentions(formats, "#/components/schemas/Model3dFormat"), "{formats}");
        assert_eq!(formats["type"], json!(["array", "null"]), "{formats}");
        // Inlined rather than a `$ref`, because utoipa drops `deprecated` from a
        // bare reference — and a deprecation nobody can see is not one.
        let legacy = property(req, "output_format");
        assert!(mentions(legacy, "\"3mf\""), "carries the vocabulary: {legacy}");
        assert!(mentions(legacy, "\"deprecated\":true"), "{legacy}");
        assert!(mentions(property(req, "topology"), "Model3dTopology"));
        assert!(mentions(property(req, "symmetry"), "Model3dSymmetry"));
    }

    #[test]
    fn model_task_is_a_named_enum_on_the_schema_and_the_capabilities() {
        let d = doc();
        let s = &d["components"]["schemas"];
        assert_eq!(
            s["ModelTask"]["enum"],
            json!(["generate", "rig", "animate", "texture", "remesh", "segment",
                   "multiview", "background_swap", "extend", "retake", "video_edit"]),
        );
        assert!(mentions(&s["ModelCapabilities"]["properties"]["task"], "#/components/schemas/ModelTask"), "{}", s["ModelCapabilities"]);
        assert!(mentions(&s["ModelSchema"]["properties"]["task"], "#/components/schemas/ModelTask"), "{}", s["ModelSchema"]);
    }

    #[test]
    fn a_3d_asset_documents_its_manifest_and_the_original_kind() {
        let d = doc();
        let s = &d["components"]["schemas"];
        assert_eq!(s["Model3dAssetKind"]["enum"], json!(["mesh", "texture", "preview", "original", "package"]));
        assert!(mentions(property(&s["Model3dAsset"], "scene"), "#/components/schemas/SceneManifest"), "{}", s["Model3dAsset"]);
        for (schema, field) in [
            ("SceneManifest", "front"), ("SceneManifest", "oriented_by"), ("SceneManifest", "extensions_used"),
            ("SceneSkin", "humanoid"), ("SceneSkin", "bind_pose"), ("SceneAnimation", "in_place"),
            ("SceneAnimation", "root_travel_m"), ("SceneBounds", "max"), ("SceneMaterial", "maps"),
        ] {
            property(&s[schema], field);
        }
        assert!(mentions(property(&s["ModelSchema"], "native_orientation"), "#/components/schemas/NativeOrientation"), "{}", s["ModelSchema"]);
        for field in ["up", "front", "evidence"] {
            property(&s["NativeOrientation"], field);
        }
    }

    /// litegen makes one image, clip or mesh per request (E3): the shared
    /// request schema says so, so a generated client can refuse `n: 2` itself.
    #[test]
    fn n_is_documented_as_one_output_per_request() {
        let d = doc();
        let n = property(&d["components"]["schemas"]["BaseGenerationRequest"], "n");
        assert_eq!(n["maximum"], json!(1), "{n}");
    }

    /// A clip lasts more than 0 s (E1, N5): the published request says so, so
    /// a generated client can refuse 0 or below itself, as the server does.
    #[test]
    fn duration_seconds_is_documented_above_zero() {
        let d = doc();
        let duration = property(&d["components"]["schemas"]["VideoGenerationRequest"], "duration_seconds");
        assert_eq!(duration["exclusiveMinimum"], json!(0), "{duration}");
    }

    /// A model listing says when a frame image sets the clip's shape (E5), so
    /// a client can hide the ratio control instead of sending a ratio the
    /// server refuses. Guards the published name and values of the field.
    #[test]
    fn a_ratio_spec_says_when_a_frame_sets_the_shape() {
        let d = doc();
        let s = &d["components"]["schemas"];
        assert_eq!(s["FramesSetShape"]["enum"], json!(["always", "without_references"]), "{}", s["FramesSetShape"]);
        let field = property(&s["ParamSpecAspectRatio"], "frames_set_shape");
        assert!(mentions(field, "#/components/schemas/FramesSetShape"), "{field}");
        assert!(!mentions(&s["ParamSpecAspectRatio"]["required"], "frames_set_shape"), "optional: {}", s["ParamSpecAspectRatio"]);
    }

    #[test]
    fn prompt_is_optional_on_the_wire() {
        // `PromptSpec.required` decides, per model. A wire-level `required`
        // would force image-to-3D callers to send `prompt: ""` to models that
        // have no prompt field at all.
        let d = doc();
        let required = d["components"]["schemas"]["BaseGenerationRequest"]["required"].clone();
        assert_eq!(required, json!(["model"]), "only the model id is structurally required");
    }

    #[test]
    fn served_only_fields_are_documented() {
        // `derivable_values` is computed by the server (the loader refuses it
        // in yaml); utoipa cannot mark an array property readOnly, so the
        // description is what tells a client it never sends it.
        let d = doc();
        let s = &d["components"]["schemas"];
        let derivable = &s["ParamSpecStringArray"]["properties"]["derivable_values"];
        assert!(mentions(derivable, "Computed\\nby the server") || mentions(derivable, "Computed by the server"), "{derivable}");
        assert!(s["ModelSchema"]["properties"]["docs_url"].is_object(), "{}", s["ModelSchema"]);
    }

    #[test]
    fn every_billed_post_documents_its_idempotency_key() {
        let d = doc();
        for path in ["/v1/images/generations", "/v1/videos/generations", "/v1/models3d/generations"] {
            let op = &d["paths"][path]["post"];
            let params = op["parameters"].as_array().cloned().unwrap_or_default();
            let key = params.iter().find(|p| p["name"] == "Idempotency-Key").unwrap_or_else(|| panic!("{path}: no Idempotency-Key"));
            assert_eq!(key["in"], "header", "{path}");
            assert_ne!(key["required"], json!(true), "{path}: the key is optional");
            assert!(op["responses"]["409"].is_object(), "{path} does not document 409");
            assert!(op["responses"]["504"].is_object(), "{path} does not document 504");
            assert!(op["responses"]["200"]["headers"]["Idempotent-Replayed"].is_object(), "{path}");
        }
        assert!(d["components"]["schemas"]["ErrorDetail"]["properties"]["vendor_job_id"].is_object());
    }

    #[test]
    fn every_generation_post_documents_the_dropped_params_header() {
        let d = doc();
        for (path, status) in [
            ("/v1/images/generations", "200"),
            ("/v1/videos/generations", "200"),
            ("/v1/models3d/generations", "200"),
            ("/v1/images/cost", "200"),
            ("/v1/videos/cost", "200"),
            ("/v1/models3d/cost", "200"),
            ("/v1/images/segmentations", "200"),
            ("/v1/images/generations", "202"),
        ] {
            let h = &d["paths"][path]["post"]["responses"][status]["headers"]["X-Litegen-Dropped-Params"];
            assert!(h.is_object(), "{path} {status} does not document X-Litegen-Dropped-Params");
        }
    }

    /// What the validator counts as a known field (E2): exactly the
    /// properties each published request schema documents, through the
    /// `allOf` and `$ref` a flattened base makes.
    #[test]
    fn request_fields_are_the_documented_properties() {
        use super::request_fields;
        let video = request_fields("VideoGenerationRequest");
        for f in ["duration_seconds", "loop", "n", "strict", "extra", "metadata", "reference_images"] {
            assert!(video.contains(f), "video lacks {f}: {video:?}");
        }
        assert!(!video.contains("duration") && !video.contains("size"), "{video:?}");
        let image = request_fields("ImageGenerationRequest");
        assert!(image.contains("size") && image.contains("response_format"), "{image:?}");
        let mesh = request_fields("Model3dGenerationRequest");
        assert!(mesh.contains("output_format") && mesh.contains("ground"), "{mesh:?}");
        let seg: Vec<&str> = request_fields("ImageSegmentationRequest").iter().map(String::as_str).collect();
        assert_eq!(seg, ["image", "metadata", "model", "prompts", "strict"]);
    }

    /// Unknown keys are found at any depth (N6) and named by their path;
    /// `extra` and `metadata` are free-form, and a key sent as `null` asks
    /// for nothing.
    #[test]
    fn unknown_fields_are_found_inside_the_requests_objects() {
        use super::{unknown_fields, UnknownField};
        let at = |path: &str, top_level: bool| UnknownField { path: path.to_string(), top_level };
        let body = json!({
            "model": "m", "prompt": "p", "duration": 10, "colour": null, "aspect_ratio": null,
            "reference_images": [{ "type": "url", "value": "u" }, { "type": "url", "value": "u", "weight": 2 }],
            "time_range": { "start_s": 0, "end_s": 1, "stop_s": 2 },
            "extra": { "anything": { "goes": 1 } }, "metadata": { "k": "v" },
        });
        assert_eq!(
            unknown_fields("VideoGenerationRequest", &body),
            vec![at("duration", true), at("reference_images[1].weight", false), at("time_range.stop_s", false)]
        );
        let mesh = json!({ "model": "m", "animations": [{ "name": "walk", "duration": 2 }] });
        assert_eq!(unknown_fields("Model3dGenerationRequest", &mesh), vec![at("animations[0].duration", false)]);
        let seg = json!({ "model": "m", "image": "x", "prompts": [{ "type": "point", "x": 1, "y": 2, "label": 1, "radius": 3 }], "prompt": "cat" });
        assert_eq!(unknown_fields("ImageSegmentationRequest", &seg), vec![at("prompt", true), at("prompts[0].radius", false)]);
    }

    /// Segmentation takes `strict` like every other generation request (E2,
    /// N3) and says it refuses an unknown field with `param_unsupported`.
    #[test]
    fn segmentation_documents_strict_and_its_refusal() {
        let d = doc();
        let s = &d["components"]["schemas"]["ImageSegmentationRequest"];
        assert!(s["properties"].get("strict").is_some_and(|p| mentions(p, "boolean")), "{s}");
        assert_eq!(s["required"], json!(["model", "image", "prompts"]));
        let bad = &d["paths"]["/v1/images/segmentations"]["post"]["responses"]["400"]["description"];
        assert!(mentions(bad, "param_unsupported"), "{bad}");
    }
    #[test]
    fn stored_file_access_is_documented() {
        let d = doc();
        let s = &d["components"]["schemas"];
        let path = &d["paths"]["/v1/apps/{app_id}/access"];
        assert!(path["get"].is_object() && path["put"].is_object(), "{path}");
        assert_eq!(s["AccessMode"]["enum"], json!(["public", "signed", "private"]));
        assert_eq!(s["OwnerScope"]["enum"], json!(["key", "app", "org"]));
        for f in ["app_id", "overrides", "effective", "updated_at"] {
            property(&s["AppAccessSettings"], f);
        }
        for f in ["mode", "owner_scope", "signed_url_ttl_seconds", "keep_outputs", "retention_days", "signed_links"] {
            property(&s["PutAppAccessRequest"], f);
            property(&s["AppAccessOverrides"], f);
            property(&s["AccessSettings"], f);
        }
        assert!(mentions(property(&s["PutAppAccessRequest"], "mode"), "#/components/schemas/AccessMode"));
        for schema in ["GenerationAsset", "Model3dAsset", "ImageResult"] {
            property(&s[schema], "expires_at");
        }
        property(&s["VideoGenerationResponse"], "video_url_expires_at");
        assert!(mentions(property(&s["AccessSettings"], "signed_links"), "#/components/schemas/SignedLinks"));
        for f in [
            "storage_access_mode", "storage_access_owner_scope", "storage_access_signed_url_ttl_seconds",
            "storage_access_keep_outputs", "storage_access_retention_days", "storage_access_signed_links",
        ] {
            property(&s["GatewaySettings"], f);
        }
        property(&s["GatewaySettingsView"], "effective_storage_access");
    }
    /// Late and async images (docs/async-images.md): the 202 a caller gets
    /// for `Prefer: respond-async`, the header that asks for it, and the
    /// generation a late 504 names.
    #[test]
    fn async_images_are_documented() {
        let d = doc();
        let post = &d["paths"]["/v1/images/generations"]["post"];
        let accepted = &post["responses"]["202"];
        assert!(mentions(accepted, "#/components/schemas/ImageGenerationAccepted"), "{accepted}");
        for h in ["Location", "Preference-Applied"] {
            assert!(accepted["headers"][h].is_object(), "the 202 documents {h}");
        }
        assert!(
            post["parameters"].as_array().is_some_and(|ps| ps.iter().any(|p| p["name"] == "Prefer" && p["in"] == "header")),
            "the Prefer header is documented"
        );
        let s = &d["components"]["schemas"];
        for f in ["id", "object", "status", "model", "created"] {
            property(&s["ImageGenerationAccepted"], f);
        }
        for f in ["generation_id", "status"] {
            property(&s["ErrorDetail"], f);
        }
    }

    /// A retired model's 410 names its replacement; an unknown one is a 404;
    /// a listed model's `task` is always sent.
    #[test]
    fn retired_models_and_model_tasks_are_documented() {
        let d = doc();
        for path in ["/v1/images/generations", "/v1/videos/generations", "/v1/models3d/generations", "/v1/images/segmentations"] {
            let responses = &d["paths"][path]["post"]["responses"];
            for status in ["404", "410"] {
                assert!(responses[status].is_object(), "{path} documents {status}");
            }
        }
        assert!(d["paths"]["/v1/models/{id}"]["get"]["responses"]["410"].is_object());
        let s = &d["components"]["schemas"];
        property(&s["ErrorDetail"], "replaced_by");
        // Listed models always carry their task; a ModelSchema is also the
        // admin API's input, where task may be left out (generate).
        let required = |schema: &str| s[schema]["required"].as_array().cloned().unwrap_or_default();
        assert!(required("ModelCapabilities").iter().any(|r| r == "task"), "{:?}", required("ModelCapabilities"));
        assert!(!required("ModelSchema").iter().any(|r| r == "task"), "{:?}", required("ModelSchema"));
    }
    #[test]
    fn gateway_settings_are_documented() {
        let d = doc();
        let s = &d["components"]["schemas"];
        for f in [
            "cache_enabled", "cache_ttl_seconds", "cache_max_items", "cache_max_bytes",
            "retry_max_retries", "retry_backoff_ms", "retry_max_backoff_ms",
            "vendor_timeout_seconds", "circuit_breaker_threshold", "circuit_breaker_open_for_seconds",
        ] {
            property(&s["GatewaySettings"], f);
        }
        for f in ["effective_cache", "cache_entries", "effective_retries", "effective_vendor_timeout_seconds", "effective_circuit_breaker"] {
            property(&s["GatewaySettingsView"], f);
        }
        for (schema, fields) in [
            ("EffectiveCache", &["enabled", "ttl_seconds", "max_items", "max_bytes"][..]),
            ("EffectiveRetries", &["max_retries", "backoff_ms", "max_backoff_ms"][..]),
            ("EffectiveBreaker", &["threshold", "open_for_seconds"][..]),
        ] {
            for f in fields {
                property(&s[schema], f);
            }
        }
        let h = &d["paths"]["/v1/images/generations"]["post"]["responses"]["200"]["headers"]["X-Litegen-Cache"];
        assert!(h.is_object(), "the image answer documents X-Litegen-Cache");
    }
}
