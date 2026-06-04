export {
  LiteGenClient,
  VideoJob,
  type LiteGenClientOptions,
  type FetchLike,
  type AuditLogEntry,
  type PaginatedResponse,
  type Generation,
  type RequestArtifact,
  type WebhookDelivery,
  type TestWebhookResponse,
  type PatchKeyBody,
  type CreateKeyBody,
  type ListAuditLogOptions,
  type GetLogsOptions,
  type ListWebhookDeliveriesOptions,
  type AuthConfigResponse,
} from "./client";
export {
  LiteGenError,
  LiteGenAPIError,
  LiteGenTimeoutError,
  LiteGenPollingTimeoutError,
} from "./errors";
export { waitForCompletion, pollVideo, type WaitForCompletionOptions } from "./polling";

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
} as const;
export type RefImageKind = (typeof RefImageKind)[keyof typeof RefImageKind];

// Convenience type aliases for the most common request/response shapes.
export type ImageGenerationRequest = components["schemas"]["ImageGenerationRequest"];
export type ImageGenerationResponse = components["schemas"]["ImageGenerationResponse"];
export type ImageResult = components["schemas"]["ImageResult"];
export type VideoGenerationRequest = components["schemas"]["VideoGenerationRequest"];
export type VideoGenerationResponse = components["schemas"]["VideoGenerationResponse"];
export type ReferenceImage = components["schemas"]["ReferenceImage"];
export type ModelInfo = components["schemas"]["ModelInfo"];
export type ModelSchema = components["schemas"]["ModelSchema"];
export type CostEstimate = components["schemas"]["CostEstimate"];
export type UsageInfo = components["schemas"]["UsageInfo"];
export type ProxyStats = components["schemas"]["ProxyStats"];
export type RequestLog = components["schemas"]["RequestLog"];
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
export type ParamSpecSeed = components["schemas"]["ParamSpecSeed"];
export type ParamSpecAspectRatio = components["schemas"]["ParamSpecAspectRatio"];
export type SizeSpec = components["schemas"]["SizeSpec"];
export type SizeSpecFreeform = components["schemas"]["SizeSpecFreeform"];
export type SizeSpecEnum = components["schemas"]["SizeSpecEnum"];
export type RefInputSpec = components["schemas"]["RefInputSpec"];
export type RefRoleSpec = components["schemas"]["RefRoleSpec"];
export type RefProviderFormat = components["schemas"]["RefProviderFormat"];
export type RefProviderFormatMultipart = components["schemas"]["RefProviderFormatMultipart"];
export type PromptSpec = components["schemas"]["PromptSpec"];
export type ModelCapabilityFlags = components["schemas"]["ModelCapabilityFlags"];
