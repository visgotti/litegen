//! OpenAPI aggregator. Drives `/openapi.json` which the first-party SDKs consume.
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "LiteGen API",
        version = env!("CARGO_PKG_VERSION"),
        description = "Universal proxy for AI image, video and 3D model generation."
    ),
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
    ),
    components(schemas(
        // Request / response types (from `types`)
        crate::types::BaseGenerationRequest,
        crate::types::ImageGenerationRequest,
        crate::types::ImageGenerationResponse,
        crate::types::ImageSegmentationRequest,
        crate::types::ImageSegmentationResponse,
        crate::types::SegmentationPrompt,
        crate::types::SegmentationMask,
        crate::capabilities::SegmentationPromptType,
        crate::capabilities::ParamSpecSegmentationPrompts,
        crate::capabilities::ParamSpecExpand,
        crate::capabilities::ParamSpecLayout,
        crate::capabilities::ParamSpecSourceVideo,
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
    )
)]
pub struct ApiDoc;

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
            json!(["glb", "gltf", "obj", "stl", "ply", "fbx", "usdz", "usda", "3mf", "dae"]),
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
        assert_eq!(s["Model3dAssetKind"]["enum"], json!(["mesh", "texture", "preview", "original"]));
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
        for path in ["/v1/images/generations", "/v1/videos/generations", "/v1/models3d/generations"] {
            let h = &d["paths"][path]["post"]["responses"]["200"]["headers"]["X-Litegen-Dropped-Params"];
            assert!(h.is_object(), "{path} does not document X-Litegen-Dropped-Params");
        }
    }
}
