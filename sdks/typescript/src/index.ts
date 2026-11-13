export {
  LiteGenClient,
  GenerationJob,
  VideoJob,
  Model3dJob,
  DROPPED_PARAMS_HEADER,
  parseDroppedParams,
  readResponseMeta,
  DEFAULT_TIMEOUT_MS,
  type LiteGenResponseMeta,
  type LiteGenResponseInfo,
  type JobSubmission,
  type IdempotencyOptions,
  type KeepOutputsOptions,
  type ImageGenerateOptions,
  type ImageSubmitOptions,
  type ImageGenerationAccepted,
  type ImageSubmitResult,
  isAccepted,
  type JobGenerateOptions,
  type ListModelsOptions,
  type Model3dConversion,
  type Model3dUnit,
  type ConvertModel3dOptions,
  type LiteGenClientOptions,
  type FetchLike,
  type AuditLogEntry,
  type PaginatedResponse,
  type Generation,
  type CancelledGeneration,
  type GenerationAsset,
  type GenerationAssetList,
  type RequestArtifact,
  type WebhookDelivery,
  type TestWebhookResponse,
  type PatchKeyBody,
  type CreateKeyBody,
  type ListAuditLogOptions,
  type ExportAuditLogOptions,
  type GetLogsOptions,
  type GetStatsOptions,
  type ExportLogsOptions,
  type ListWebhookDeliveriesOptions,
  type AuthConfigResponse,
  type OAuthRequestView,
  type ApproveOAuthRequest,
  type OAuthRedirect,
  type ConnectedApp,
  type RevokedGrant,
  type CredentialFieldSpec,
  type ProviderCatalogEntry,
  type OrgAllowedModels,
  type AppModelAccess,
  type SetOrgAllowedModelsRequest,
  type SetAppModelAccessRequest,
} from "./client";
export {
  LiteGenError,
  LiteGenAPIError,
  LiteGenTimeoutError,
  LiteGenPollingTimeoutError,
} from "./errors";
export {
  waitForCompletion,
  pollVideo,
  poll3d,
  waitForModel3d,
  waitForJob,
  pollJob,
  JOB_DEFAULT_TIMEOUT_MS,
  MODEL3D_DEFAULT_TIMEOUT_MS,
  type WaitForCompletionOptions,
} from "./polling";
export { VERSION } from "./version";

// Re-export raw schema types for advanced use.
export type { components, operations, paths } from "./generated/schema";

import type { components } from "./generated/schema";

// Convenience enum-like constants matching the API's snake_case JSON values.
export const GenerationStatus = {
  Pending: "pending",
  Processing: "processing",
  Completed: "completed",
  Failed: "failed",
  Cancelled: "cancelled",
} as const;
export type GenerationStatus = (typeof GenerationStatus)[keyof typeof GenerationStatus];

export const MediaType = {
  Image: "image",
  Video: "video",
  Model3d: "model3d",
} as const;
export type MediaType = (typeof MediaType)[keyof typeof MediaType];

export const CostSource = {
  Dynamic: "dynamic",
  Estimated: "estimated",
} as const;
export type CostSource = (typeof CostSource)[keyof typeof CostSource];

export const RoutingStrategy = {
  Fallback: "fallback",
  WeightedRoundRobin: "weighted_round_robin",
  LowestCost: "lowest_cost",
  LowestLatency: "lowest_latency",
} as const;
export type RoutingStrategy = (typeof RoutingStrategy)[keyof typeof RoutingStrategy];

export const RefImageKind = {
  Base64: "base64",
  Url: "url",
  Blob: "blob",
  /** A stored litegen image generation's id; the server reads the image back. */
  Generation: "generation",
} as const;
export type RefImageKind = (typeof RefImageKind)[keyof typeof RefImageKind];

/**
 * What one file in a 3D result is: the mesh itself, a texture map, a 2D preview
 * render, or `original` — the vendor's own file, kept beside a mesh litegen
 * changed to deliver it in glTF's conventions.
 */
export const Model3dAssetKind = {
  Mesh: "mesh",
  Texture: "texture",
  Preview: "preview",
  Original: "original",
  Package: "package",
} as const;
export type Model3dAssetKind = (typeof Model3dAssetKind)[keyof typeof Model3dAssetKind];

// Convenience type aliases for the most common request/response shapes.
export type ImageGenerationRequest = components["schemas"]["ImageGenerationRequest"];
export type ImageGenerationResponse = components["schemas"]["ImageGenerationResponse"];
export type ImageResult = components["schemas"]["ImageResult"];
/**
 * A rectangle of the init image (`region`): pixels, top-left origin, ends
 * exclusive. A box-native model receives it as its own box, a mask-only one
 * drawn as its mask; with a `mask` too, the edit area is where they overlap.
 */
export type Region = components["schemas"]["Region"];
/** Pixels to add to each side of the init image's canvas; the source lands at (left, top). */
export type Expand = components["schemas"]["Expand"];
/** One box of a text-to-image layout, in canvas fractions: an object (`prompt`) or words (`text`). */
export type LayoutItem = components["schemas"]["LayoutItem"];
export type LayoutKind = components["schemas"]["LayoutKind"];
/** A window of a clip, in seconds (`retake`, `video_edit`). */
export type TimeRange = components["schemas"]["TimeRange"];
/** A native pixel-art model's output edge, in pixels. */
export type PixelSize = components["schemas"]["PixelSize"];
/** One stretch of a video operation's clip: from the source or generated. */
export type VideoSegment = components["schemas"]["VideoSegment"];
export type ExtendOutput = components["schemas"]["ExtendOutput"];
/** `POST /v1/images/segmentations` (PixelGenie L7): an image and what to select in it. */
export type ImageSegmentationRequest = components["schemas"]["ImageSegmentationRequest"];
/** One mask per object selected, best first. */
export type ImageSegmentationResponse = components["schemas"]["ImageSegmentationResponse"];
/** A point (`label` 1 selects, 0 leaves out), a box, or text naming what to select. */
export type SegmentationPrompt = components["schemas"]["SegmentationPrompt"];
/** A grayscale PNG the image's size (white = selected), its score and its box. */
export type SegmentationMask = components["schemas"]["SegmentationMask"];
/** The prompt types a model segments by: its `capabilities.segmentation_prompts`. */
export type SegmentationPromptType = components["schemas"]["SegmentationPromptType"];
/**
 * A view of a subject, named for the side the camera sees. Its yaw
 * (`ImageResult.yaw_deg`) is the camera's position around +Y from the
 * subject's front (+Z) towards +X: front 0, left 90, back 180, right 270, the
 * diagonals between; top and bottom have none. A multiview image model takes
 * `views` from this list; a multiview-to-3D model takes `view-<name>` references.
 */
export type ImageView = components["schemas"]["ImageView"];
/**
 * Where a model is in its vendor's lifecycle (`ModelInfo.status`, and
 * `ModelSchema.status`, derived from its dates and ignored when a schema is
 * posted back): a `deprecated` model is still served until the day after its
 * `retires_at`; `replaced_by` names the catalog model to move to.
 */
export type ModelStatus = components["schemas"]["ModelStatus"];
/** A video's camera (`camera`): `fixed` holds the shot still (`supports_fixed_camera`). */
export type VideoCamera = components["schemas"]["VideoCamera"];
/** What a finished video is, read from its bytes: length, rate, size, audio. */
export type VideoMedia = components["schemas"]["VideoMedia"];
/** An image container (`output_format`): one of the model's `capabilities.output_formats`. */
export type ImageOutputFormat = components["schemas"]["ImageOutputFormat"];
/** An image's background: `transparent` is a real alpha channel (needs PNG or WebP). */
export type ImageBackground = components["schemas"]["ImageBackground"];
/** What a vendor said when it refused or failed a request (`LiteGenAPIError.providerError`). */
export type VendorError = components["schemas"]["VendorError"];
/**
 * Where a video model's `keyframe` references may sit
 * (`capabilities.keyframe_timing`): `any` time each, or `even` spacing from
 * the start of the clip to its end.
 */
export type KeyframeTiming = components["schemas"]["KeyframeTiming"];
/**
 * When a video model's frame image sets the clip's shape
 * (`params.aspect_ratio.frames_set_shape` in `models.get(id)`): with a
 * `first_frame`, `last_frame` or `init` image its vendor takes no ratio, so
 * `aspect_ratio` is refused (`param_unsupported`) when `strict` and dropped
 * and named otherwise. `always`: with any frame image; `without_references`:
 * unless a `reference` image goes too, which sends the request to a vendor
 * mode that takes a ratio. Absent: the ratio applies with any image.
 */
export type FramesSetShape = components["schemas"]["FramesSetShape"];
/**
 * One rule of a model's `pricing.variable_pricing`: applied in order to a
 * running price that starts at `base_cost_usd`, when every `when` entry
 * matches the request (`"*"` for any value present). `/cost`, the quota
 * reservation and the charge all use them.
 */
export type PriceRule = components["schemas"]["PriceRule"];
/** Which pixels of a mask edit. Callers always send `white_edits`; litegen converts. */
export type MaskConvention = components["schemas"]["MaskConvention"];
/** How a model receives a `region`: its own `box`, or drawn as its `mask`. */
export type RegionInput = components["schemas"]["RegionInput"];
/** What a vendor promises about pixels outside an edit. */
export type PreservesUnmasked = components["schemas"]["PreservesUnmasked"];
export type VideoGenerationRequest = components["schemas"]["VideoGenerationRequest"];
export type VideoGenerationResponse = components["schemas"]["VideoGenerationResponse"];
export type Model3dGenerationRequest = components["schemas"]["Model3dGenerationRequest"];
export type Model3dGenerationResponse = components["schemas"]["Model3dGenerationResponse"];
export type Model3dAsset = components["schemas"]["Model3dAsset"];
/**
 * What litegen measured in a delivered mesh (`Model3dAsset.scene`), read from
 * the file's own bytes: metres, `+y` up, facing `+z` — or `front: "unverified"`
 * for a model whose orientation nobody has proven yet.
 */
export type SceneManifest = components["schemas"]["SceneManifest"];
export type SceneBounds = components["schemas"]["SceneBounds"];
/** One part of a segmented mesh: a glTF node, named with the vendor's label or `part_<n>`. */
export type ScenePart = components["schemas"]["ScenePart"];
export type SceneMaterial = components["schemas"]["SceneMaterial"];
/** A delivered mesh's skeleton, with its VRM 1.0 `humanoid` map when litegen recognises the rig. */
export type SceneSkin = components["schemas"]["SceneSkin"];
export type SceneAnimation = components["schemas"]["SceneAnimation"];
/** A 3D model's proven native axes (`ModelSchema.native_orientation`). */
export type NativeOrientation = components["schemas"]["NativeOrientation"];
/**
 * Every mesh container a 3D request may name in `output_formats`. Which of
 * them one model can deliver is that model's `params.output_formats.enum_values`
 * (`models.get(id)`), and which of those litegen converts locally — so that any
 * subset of them costs one provider job — is its `derivable_values`. Both are
 * per model and change as litegen learns new containers; read them, don't
 * hard-code them.
 */
export type Model3dFormat = components["schemas"]["Model3dFormat"];
/** What `models3d.convertFormats()` returns: the converter's containers and its input cap. */
export type Model3dConvertFormats = components["schemas"]["Model3dConvertFormats"];
/** Face topology a 3D request may ask for (`topology`). */
export type Model3dTopology = components["schemas"]["Model3dTopology"];
/** Symmetry handling a 3D request may ask for (`symmetry`). */
export type Model3dSymmetry = components["schemas"]["Model3dSymmetry"];
/** The skeleton a rig model builds (`skeleton`); a model lists the ones it rigs in `params.skeleton.enum_values`. */
export type Model3dSkeleton = components["schemas"]["Model3dSkeleton"];
/** The joint names a rig is delivered with (`joint_naming`); `vendor` keeps the vendor's own. */
export type Model3dJointNaming = components["schemas"]["Model3dJointNaming"];
export type Model3dFbxColorSpace = components["schemas"]["Model3dFbxColorSpace"];
export type Model3dFormatSource = components["schemas"]["Model3dFormatSource"];
/**
 * What delivery does with a KTX2 (Basis Universal) texture (`ktx2_textures`):
 * `png` (the default) re-encodes it as PNG in every container; `keep` leaves
 * it in `glb` and `gltf`, which then require `KHR_texture_basisu`.
 */
export type Model3dKtx2Textures = components["schemas"]["Model3dKtx2Textures"];
/**
 * How a delivered or read FBX or USD stores its light strengths
 * (`fbx_light_units`, `usd_light_units`): `photometric` (the default; the
 * scene's own, glTF's candela and lux) or `blender` (Blender's `energy`, so
 * the lights open in Blender as bright as the glTF does).
 */
export type Model3dLightUnits = components["schemas"]["Model3dLightUnits"];
/**
 * How a Collada (`dae`) file litegen writes stores a mesh's morph targets
 * (`dae_morphs`): `bake` (the default; each node's mesh at that node's
 * weights, which every Collada reader opens) or `keep` (`<morph>` controllers
 * with their weight animation, which SceneKit and Godot read and three.js and
 * assimp refuse).
 */
export type Model3dDaeMorphs = components["schemas"]["Model3dDaeMorphs"];
/**
 * What the root layer of a `usdz` litegen writes is (`usdz_root`): `usda`
 * (the default; text, which `usdchecker --arkit` accepts and `<model-viewer>`
 * hands to AR Quick Look) or `usdc` (the binary crate, as Apple's own
 * packagers write it). A request that keeps nothing can't ask for `usdc`
 * (`option_needs_keeping`).
 */
export type Model3dUsdzRoot = components["schemas"]["Model3dUsdzRoot"];
/**
 * Which animation clips a USD file litegen writes holds (`usd_clips`): `all`
 * (every clip as an alternative, chosen by a variant set `Animation`; rigid
 * node motion is the first clip's only, the others' named in the losses) or
 * `first` (the first, the rest named in the losses). Unset is the
 * container's default: `all` for `usda` and `usdc`, `first` for `usdz`. A
 * request that keeps nothing can't ask for `all` (`option_needs_keeping`).
 */
export type Model3dUsdClips = components["schemas"]["Model3dUsdClips"];
/**
 * One clip an `animate` model makes (`animations[]`): exactly one of `preset`
 * and `prompt`, delivered as a glTF animation, in order, named its `name`. A
 * clip that leaves `name` out (or sends `null`) is named by litegen: after its
 * canonical preset (`walk`) or else its preset id, a prompt clip `clip-<n>` by
 * its place, with `-2`, `-3`, … on a name already taken.
 */
export type AnimationRequest = components["schemas"]["AnimationRequest"];
/** The canonical presets; a model maps the ones it plays onto its own ids. */
export type Model3dAnimationPreset = components["schemas"]["Model3dAnimationPreset"];
/** An animate model's `params.animations`: its presets, limits and how it keeps clips in place. */
export type ParamSpecAnimationList = components["schemas"]["ParamSpecAnimationList"];
export type AnimationPresetSpec = components["schemas"]["AnimationPresetSpec"];
/**
 * What a catalog model does (`task` on its schema, `capabilities.task` in the
 * listing): `generate`, or an operation on an existing asset such as `rig` or
 * `animate`, submitted to its modality's usual generation endpoint.
 */
export type ModelTask = components["schemas"]["ModelTask"];
/**
 * The mesh an operation (`rig`, `animate`, …) works on: a litegen generation
 * id to chain a step onto an earlier one, or a URL / base64 file of your own
 * naming its `format`. Required by every model whose `task` is not `generate`.
 */
export type SourceRef = components["schemas"]["SourceRef"];
/** How a {@link SourceRef}'s `value` is read. */
export type SourceKind = components["schemas"]["SourceKind"];
export type ReferenceImage = components["schemas"]["ReferenceImage"];
export type ModelInfo = components["schemas"]["ModelInfo"];
/**
 * One model's full schema (`models.get(id)`), also the admin API's input.
 * `litegen_params` (read-only) holds the request fields litegen applies to the
 * model itself, in the same `ParamSpec` shape as `params`; `extra_params`
 * types the `extra_allowlist` keys.
 */
export type ModelSchema = components["schemas"]["ModelSchema"];
export type CostEstimate = components["schemas"]["CostEstimate"];
export type UsageInfo = components["schemas"]["UsageInfo"];
export type ProxyStats = components["schemas"]["ProxyStats"];
export type RequestLog = components["schemas"]["RequestLog"];
/** One entry of `health.check()`: a provider, the `modality` it checked, and how it answered. */
export type ProviderHealth = components["schemas"]["ProviderHealth"];
export type ApiKeyInfo = components["schemas"]["ApiKeyInfo"];
export type ApiKeyCreatedResponse = components["schemas"]["ApiKeyCreatedResponse"];
export type HealthResponse = components["schemas"]["HealthResponse"];
export type LivenessResponse = components["schemas"]["LivenessResponse"];

// Tenancy (orgs / apps / members / provider credentials).
export type OrgView = components["schemas"]["OrgView"];
export type OrgSummary = components["schemas"]["OrgSummary"];
export type MemberView = components["schemas"]["MemberView"];
export type Organization = components["schemas"]["Organization"];
export type Application = components["schemas"]["Application"];
export type OrganizationMember = components["schemas"]["OrganizationMember"];
export type ProviderCredentialInfo = components["schemas"]["ProviderCredentialInfo"];
export type AppStorageInfo = components["schemas"]["AppStorageInfo"];
export type PutAppStorageRequest = components["schemas"]["PutAppStorageRequest"];
export type AccessMode = components["schemas"]["AccessMode"];
export type OwnerScope = components["schemas"]["OwnerScope"];
export type AccessSettings = components["schemas"]["AccessSettings"];
export type AppAccessSettings = components["schemas"]["AppAccessSettings"];
export type PutAppAccessRequest = components["schemas"]["PutAppAccessRequest"];
export type LinksRevoked = components["schemas"]["LinksRevoked"];
export type SignedLinks = components["schemas"]["SignedLinks"];
export type CreateOrgRequest = components["schemas"]["CreateOrgRequest"];
export type UpdateOrgRequest = components["schemas"]["UpdateOrgRequest"];
export type CreateAppRequest = components["schemas"]["CreateAppRequest"];
export type UpdateAppRequest = components["schemas"]["UpdateAppRequest"];
export type AddMemberRequest = components["schemas"]["AddMemberRequest"];
export type UpdateMemberRequest = components["schemas"]["UpdateMemberRequest"];
export type OrgTransferOwnerRequest = components["schemas"]["OrgTransferOwnerRequest"];
export type CreateProviderCredentialRequest =
  components["schemas"]["CreateProviderCredentialRequest"];

// Capability schema types (response of `GET /v1/models/{id}`).
export type ParamSpec = components["schemas"]["ParamSpec"];
export type ParamSpecBool = components["schemas"]["ParamSpecBool"];
export type ParamSpecInt = components["schemas"]["ParamSpecInt"];
export type ParamSpecFloat = components["schemas"]["ParamSpecFloat"];
export type ParamSpecString = components["schemas"]["ParamSpecString"];
/** A set-valued param (`output_formats`): `enum_values`, `max_items`, and on 3D models `derivable_values`. */
export type ParamSpecStringArray = components["schemas"]["ParamSpecStringArray"];
export type ParamSpecSeed = components["schemas"]["ParamSpecSeed"];
export type ParamSpecAspectRatio = components["schemas"]["ParamSpecAspectRatio"];
export type SizeSpec = components["schemas"]["SizeSpec"];
export type SizeSpecFreeform = components["schemas"]["SizeSpecFreeform"];
export type SizeSpecEnum = components["schemas"]["SizeSpecEnum"];
export type RefInputSpec = components["schemas"]["RefInputSpec"];
export type RefRoleSpec = components["schemas"]["RefRoleSpec"];
export type RefProviderFormat = components["schemas"]["RefProviderFormat"];
export type RefProviderFormatMultipart = components["schemas"]["RefProviderFormatMultipart"];
/**
 * A model's prompt rules (`prompt` in `models.get(id)`). `required_for_text_to_video`:
 * a video model needs the prompt without a `first_frame` or `init` image
 * (`prompt_required` before anything is quoted), though not with one.
 */
export type PromptSpec = components["schemas"]["PromptSpec"];
export type ModelCapabilityFlags = components["schemas"]["ModelCapabilityFlags"];
/**
 * The `capabilities` projection on each `GET /v1/models` entry (incl. the 3D
 * `supports_*`, `output_formats`, `max_polycount`, and `max_request_bytes`,
 * on every model: the largest generation or quote body the server reads
 * before a 413).
 */
export type ModelCapabilities = components["schemas"]["ModelCapabilities"];

// Management API (budgets/limits, end users, spend, gateway admin).
export type EntityType = components["schemas"]["EntityType"];
export type EntityLimits = components["schemas"]["EntityLimits"];
export type LimitsInput = components["schemas"]["LimitsInput"];
export type KeyActiveResponse = components["schemas"]["KeyActiveResponse"];
export type DeletedResponse = components["schemas"]["DeletedResponse"];
export type EndUser = components["schemas"]["EndUser"];
export type EndUserView = components["schemas"]["EndUserView"];
export type CreateEndUserRequest = components["schemas"]["CreateEndUserRequest"];
export type UpdateEndUserRequest = components["schemas"]["UpdateEndUserRequest"];
export type SpendGroupBy = components["schemas"]["SpendGroupBy"];
export type SpendReportRow = components["schemas"]["SpendReportRow"];
export type SpendReportResponse = components["schemas"]["SpendReportResponse"];
export type SpendLogEntry = components["schemas"]["SpendLogEntry"];
export type SpendLogsResponse = components["schemas"]["SpendLogsResponse"];
export type ModelSource = components["schemas"]["ModelSource"];
export type AdminModelView = components["schemas"]["AdminModelView"];
export type ProviderSettings = components["schemas"]["ProviderSettings"];
export type PutProviderConfigRequest = components["schemas"]["PutProviderConfigRequest"];
export type ProviderConfigView = components["schemas"]["ProviderConfigView"];
export type RouteDeployment = components["schemas"]["RouteDeployment"];
export type RouteSpec = components["schemas"]["RouteSpec"];
export type RoutesDocument = components["schemas"]["RoutesDocument"];
export type GatewaySettings = components["schemas"]["GatewaySettings"];
export type GatewaySettingsView = components["schemas"]["GatewaySettingsView"];
export type {
  ListLimitsOptions,
  EndUserOptions,
  SpendFilterOptions,
  SpendReportOptions,
  SpendLogsOptions,
} from "./client";
