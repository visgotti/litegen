import type { components } from "./generated/schema";
import { LiteGenAPIError, LiteGenTimeoutError } from "./errors";
import {
  waitForCompletion,
  pollVideo,
  poll3d,
  waitForModel3d,
  type WaitForCompletionOptions,
} from "./polling";

type Schemas = components["schemas"];
type ImageRequest = Schemas["ImageGenerationRequest"];
type ImageResponse = Schemas["ImageGenerationResponse"];
type VideoRequest = Schemas["VideoGenerationRequest"];
type VideoResponse = Schemas["VideoGenerationResponse"];
type Model3dRequest = Schemas["Model3dGenerationRequest"];
type Model3dResponse = Schemas["Model3dGenerationResponse"];
type CostEstimate = Schemas["CostEstimate"];
type ModelInfo = Schemas["ModelInfo"];
type ModelSchema = Schemas["ModelSchema"];
type ModelListResponse = Schemas["ModelListResponse"];
type HealthResponse = Schemas["HealthResponse"];
type LivenessResponse = Schemas["LivenessResponse"];
type ProxyStats = Schemas["ProxyStats"];
type RequestLog = Schemas["RequestLog"];
type ApiKeyInfo = Schemas["ApiKeyInfo"];
type ApiKeyDetail = Schemas["ApiKeyDetail"];
type ApiKeyListResponse = Schemas["ApiKeyListResponse"];
type ApiKeyCreatedResponse = Schemas["ApiKeyCreatedResponse"];
type RotatedKeyResponse = Schemas["RotatedKeyResponse"];
type RevokeKeyResponse = Schemas["RevokeKeyResponse"];
type CacheClearedResponse = Schemas["CacheClearedResponse"];
// Auth + user types
type SignupRequest = Schemas["SignupRequest"];
type LoginRequest = Schemas["LoginRequest"];
type AuthResponse = Schemas["AuthResponse"];
type CsrfResponse = Schemas["CsrfResponse"];
/** Public auth-method discovery (`GET /v1/auth/config`). */
export type AuthConfigResponse = Schemas["AuthConfigResponse"];
type PasswordResetRequestBody = Schemas["PasswordResetRequestBody"];
type PasswordResetConfirmBody = Schemas["PasswordResetConfirmBody"];
type InvitationView = Schemas["InvitationView"];
type AcceptInvitationRequest = Schemas["AcceptInvitationRequest"];
type InviteRequest = Schemas["InviteRequest"];
type InviteResponse = Schemas["InviteResponse"];
type PublicUser = Schemas["PublicUser"];
type PatchUserRequest = Schemas["PatchUserRequest"];
type TransferOwnerRequest = Schemas["TransferOwnerRequest"];
type AccountUser = Schemas["AccountUser"];
type PatchAccountRequest = Schemas["PatchAccountRequest"];
type SessionInfo = Schemas["SessionInfo"];
// Tenancy (orgs / apps / members / provider credentials)
type OrgView = Schemas["OrgView"];
type OrgSummary = Schemas["OrgSummary"];
type MemberView = Schemas["MemberView"];
type Application = Schemas["Application"];
type ProviderCredentialInfo = Schemas["ProviderCredentialInfo"];
type AppStorageInfo = Schemas["AppStorageInfo"];
type PutAppStorageRequest = Schemas["PutAppStorageRequest"];
type CreateOrgRequest = Schemas["CreateOrgRequest"];
type UpdateOrgRequest = Schemas["UpdateOrgRequest"];
type CreateAppRequest = Schemas["CreateAppRequest"];
type UpdateAppRequest = Schemas["UpdateAppRequest"];
type AddMemberRequest = Schemas["AddMemberRequest"];
type UpdateMemberRequest = Schemas["UpdateMemberRequest"];
type OrgTransferOwnerRequest = Schemas["OrgTransferOwnerRequest"];
type CreateProviderCredentialRequest = Schemas["CreateProviderCredentialRequest"];
// Management (budgets/limits, end users, spend, gateway admin)
type EntityLimits = Schemas["EntityLimits"];
type LimitsInput = Schemas["LimitsInput"];
type LimitsListResponse = Schemas["LimitsListResponse"];
type EntityType = Schemas["EntityType"];
type KeyActiveResponse = Schemas["KeyActiveResponse"];
type DeletedResponse = Schemas["DeletedResponse"];
type EndUserView = Schemas["EndUserView"];
type EndUserListResponse = Schemas["EndUserListResponse"];
type CreateEndUserRequest = Schemas["CreateEndUserRequest"];
type UpdateEndUserRequest = Schemas["UpdateEndUserRequest"];
type SpendGroupBy = Schemas["SpendGroupBy"];
type SpendReportResponse = Schemas["SpendReportResponse"];
type SpendLogsResponse = Schemas["SpendLogsResponse"];
type AdminModelView = Schemas["AdminModelView"];
type AdminModelListResponse = Schemas["AdminModelListResponse"];
type ProviderConfigView = Schemas["ProviderConfigView"];
type ProviderConfigListResponse = Schemas["ProviderConfigListResponse"];
type PutProviderConfigRequest = Schemas["PutProviderConfigRequest"];
type RoutesDocument = Schemas["RoutesDocument"];
type RouteSpec = Schemas["RouteSpec"];
type GatewaySettings = Schemas["GatewaySettings"];
type GatewaySettingsView = Schemas["GatewaySettingsView"];

export interface OrgAllowedModels {
  models: string[];
}

export interface AppModelAccess {
  mode: 'all' | 'select';
  models: string[];
}

export interface SetOrgAllowedModelsRequest {
  models: string[];
}

export interface SetAppModelAccessRequest {
  mode: 'all' | 'select';
  models?: string[];
}

interface PaginatedLogs {
  data: RequestLog[];
  total: number;
  page: number;
  per_page: number;
  total_pages: number;
}

// ─── Types not yet in generated schema ──────────────────────────────────────

export interface AuditLogEntry {
  id: string;
  actor_key_id: string | null;
  actor_label: string;
  action: string;
  target_type: string;
  target_id: string;
  before_json: string | null;
  after_json: string | null;
  created_at: string;
}

export interface PaginatedResponse<T> {
  data: T[];
  total: number;
  page: number;
  per_page: number;
  total_pages: number;
}

/**
 * A DB-backed generation row (image, video, or 3D model). Mirrors the backend
 * `Generation` struct in litegen-core (`src/types/mod.rs`) exactly — field names
 * and nullability match the serialized JSON. The real failure reason is
 * `error_message` (there is no `error` field), and there is no `request_body`.
 */
export interface Generation {
  /** Locally-minted ID (e.g. "litegen-vid-<uuid>"). */
  id: string;
  /** API key that submitted this generation; null when the master key was used. */
  key_id: string | null;
  model: string;
  provider: string;
  /** "image" | "video" | "model3d". */
  media_type: string;
  status: "pending" | "processing" | "completed" | "failed" | "cancelled";
  /** Progress percentage (0–100). */
  progress: number;
  /** Provider-assigned job ID used for polling. */
  provider_job_id?: string | null;
  /** Final result URL when completed. */
  result_url?: string | null;
  /** Failure reason when status is "failed". */
  error_message?: string | null;
  cost_usd: number;
  created_at: string;
  completed_at?: string | null;
  /** Arbitrary JSON metadata. */
  metadata?: Record<string, unknown> | null;
  /** Owning tenant (organization). Absent for pre-tenancy / untenanted rows. */
  org_id?: string;
  /** Owning application. Absent for pre-tenancy / untenanted rows. */
  app_id?: string;
}

export interface RequestArtifact {
  request_id: string;
  media_type: string;
  prompt: string | null;
  negative_prompt: string | null;
  params_json: Record<string, unknown> | null;
  refs_meta_json: unknown[] | null;
  output_kind: string;
  output_value: string | null;
  output_mime: string | null;
  output_truncated: boolean;
  error_message: string | null;
  created_at: string;
}

export interface WebhookDelivery {
  id: string;
  key_id: string;
  generation_id: string;
  url: string;
  attempt_number: number;
  status_code: number | null;
  success: boolean;
  response_body: string | null;
  error_message: string | null;
  payload_json: string;
  created_at: string;
}

export interface TestWebhookResponse {
  delivered: boolean;
  status_code: number | null;
  error: string | null;
}

export interface PatchKeyBody {
  name?: string;
  token_quota?: number | null;
  rpm_limit?: number | null;
  /** CSV string e.g. "generate,read,admin" */
  scopes?: string;
  webhook_url?: string | null;
}

export interface CreateKeyBody {
  name: string;
  token_quota?: number;
  rpm_limit?: number;
  /** CSV string e.g. "generate,read" */
  scopes?: string;
  webhook_url?: string;
  expires_at?: string;
}

export interface ListAuditLogOptions {
  page?: number;
  per_page?: number;
  actor_key_id?: string;
  action?: string;
  from?: string;
  to?: string;
}

/** Filter options for `audit.exportCsv` (same filters as `audit.list`, sans pagination). */
export type ExportAuditLogOptions = Omit<ListAuditLogOptions, "page" | "per_page">;

export interface GetLogsOptions {
  page?: number;
  per_page?: number;
  model?: string;
  provider?: string;
  status?: string;
  from?: string;
  to?: string;
}

/** Filter options for `logs.exportCsv` (same filters as `logs.list`, sans pagination). */
export type ExportLogsOptions = Omit<GetLogsOptions, "page" | "per_page">;

export interface ListWebhookDeliveriesOptions {
  page?: number;
  per_page?: number;
}

/** One field the dashboard must collect for a provider credential, per pool entry. */
export interface CredentialFieldSpec {
  /** JSON key within each pool entry (e.g. "key", "key_id", "key_secret", "region"). */
  key: string;
  /** Human-readable label for the input. */
  label: string;
  /** Whether the value is secret and should be rendered masked. */
  secret: boolean;
  /** Whether the field may be left empty. */
  optional?: boolean;
}

/** Describes a provider and how to render its credential form (from GET /v1/providers). */
export interface ProviderCatalogEntry {
  /** Provider id (e.g. "openai", "bedrock"). */
  name: string;
  /** Which media this provider serves — any of "image", "video", "model3d". */
  modalities: string[];
  /** Coarse auth scheme: "api_key" | "aws_sigv4" | "tencent_tc3" | "kling_jwt". */
  auth_scheme: string;
  /** Which credentials array to submit: "api_keys" (bearer) or "credential_sets" (signing). */
  pool_field: string;
  /** Fields to collect per pool entry, besides the universal weight/label. */
  fields: CredentialFieldSpec[];
}

/** A request body sent as raw bytes rather than JSON. */
type RawBody = Blob | ArrayBuffer | ArrayBufferView;

function isRawBody(body: unknown): body is RawBody {
  return (
    (typeof Blob !== "undefined" && body instanceof Blob) ||
    body instanceof ArrayBuffer ||
    ArrayBuffer.isView(body)
  );
}

/** What `models3d.convert` hands back. */
export interface Model3dConversion {
  /** The converted file; `blob.type` is the target's Content-Type (e.g. `model/obj`). */
  blob: Blob;
  /** The container it is in — the `to` that was asked for. */
  format: Schemas["Model3dFormat"];
  /**
   * A file name for it: the server's Content-Disposition (`model.<to>`), or the
   * same name built locally when a cross-origin response hides that header.
   */
  filename: string;
  /**
   * What this file lost on the way to the target container, from the
   * `X-Litegen-Conversion-Losses` response header — e.g. `["materials", "uvs"]`. Worked
   * out from the input's actual content, not the format pair: a plain
   * untextured mesh to STL loses nothing. Known kinds: `animation`, `skins`,
   * `morph_targets`, `normals`, `uvs`, `vertex_colors`, `textures`,
   * `image_formats`, `materials`, `alpha`, `hierarchy`, `meshes`,
   * `texture_sampling`, `texture_transforms`, `geometry`; a kind added later
   * passes through as-is. Empty when nothing was lost (or by a server that
   * does not report losses yet).
   */
  losses: string[];
}

/** Options for `models3d.convert`. Which pairs work: `models3d.convertFormats()`. */
export interface ConvertModel3dOptions {
  /** The container the input is in. A file that is not one is a 400 `format_mismatch`. */
  from: Schemas["Model3dFormat"];
  /** The container to convert it to. */
  to: Schemas["Model3dFormat"];
  signal?: AbortSignal;
}

/** Response header naming what a conversion lost, comma-joined (omitted when nothing was). */
const CONVERSION_LOSSES_HEADER = "x-litegen-conversion-losses";

/** `filename="model.obj"` out of a Content-Disposition value, if it has one. */
function dispositionFilename(value: string | null): string | null {
  const m = value ? /filename="?([^";]+)"?/i.exec(value) : null;
  return m?.[1] ?? null;
}

/** Filters for `models.list` (`GET /v1/models`). */
export interface ListModelsOptions {
  /** Only models of this family: `"image"`, `"video"` or `"model3d"`. */
  media_type?: Schemas["MediaType"];
}

// ─── Lax-mode dropped params ─────────────────────────────────────────────────

/**
 * Response header the generation POSTs set when `strict: false` made the server
 * drop params the model does not support instead of rejecting the request.
 */
export const DROPPED_PARAMS_HEADER = "x-litegen-dropped-params";

/**
 * Parse an `X-Litegen-Dropped-Params` value — a comma-joined list such as
 * `"rig,symmetry,reference_images[view-top]"` — into its entries. A missing or
 * empty header means nothing was dropped.
 */
export function parseDroppedParams(value: string | null | undefined): string[] {
  if (!value) return [];
  return value.split(",").map((p) => p.trim()).filter((p) => p !== "");
}

// ─── Client options ──────────────────────────────────────────────────────────

export type FetchLike = typeof fetch;

export interface LiteGenClientOptions {
  baseUrl?: string;
  /** Static API key — set as Bearer token on every request. */
  apiKey?: string;
  /** Dynamic API-key getter — called per-request; result set as Bearer when non-empty. */
  getAuthToken?: () => string | undefined;
  /** Passed as `credentials` option on every fetch call (default: "same-origin"). */
  credentials?: RequestCredentials;
  /**
   * Called before non-GET requests.  If it returns a string the value is sent
   * as the `X-CSRF-Token` header.
   */
  getCsrfToken?: () => Promise<string | undefined>;
  /**
   * Called after a non-2xx response before the error is thrown.
   * Receives HTTP status and the parsed body (or null).
   */
  onError?: (status: number, body: unknown) => void;
  fetch?: FetchLike;
  timeoutMs?: number;
  defaultHeaders?: Record<string, string>;
  /** Initial active organization id — sent as `X-Litegen-Org-Id`. */
  activeOrgId?: string;
  /** Initial active application id — sent as `X-Litegen-App-Id`. */
  activeAppId?: string;
}

// ─── Client ──────────────────────────────────────────────────────────────────

export class LiteGenClient {
  private readonly baseUrl: string;
  private readonly apiKey?: string;
  private readonly getAuthToken?: () => string | undefined;
  private readonly credentials: RequestCredentials;
  private readonly getCsrfToken?: () => Promise<string | undefined>;
  private readonly onError?: (status: number, body: unknown) => void;
  private readonly fetchImpl: FetchLike;
  private readonly timeoutMs: number;
  private readonly defaultHeaders: Record<string, string>;
  /** Active tenant context sent as `X-Litegen-Org-Id` / `X-Litegen-App-Id`. */
  private activeOrgId?: string;
  private activeAppId?: string;

  readonly images: ImagesNamespace;
  readonly videos: VideosNamespace;
  readonly models3d: Models3dNamespace;
  readonly models: ModelsNamespace;
  readonly providers: ProvidersNamespace;
  readonly health: HealthNamespace;
  readonly stats: StatsNamespace;
  readonly logs: LogsNamespace;
  readonly keys: KeysNamespace;
  readonly cache: CacheNamespace;
  readonly auth: AuthNamespace;
  readonly users: UsersNamespace;
  readonly account: AccountNamespace;
  readonly audit: AuditNamespace;
  readonly generations: GenerationsNamespace;
  readonly orgs: OrgsNamespace;
  readonly apps: AppsNamespace;
  /** Budgets and limits across keys, orgs, apps and members. */
  readonly limits: LimitsNamespace;
  /** End users (your callers' customers), named per request with `x-litegen-end-user`. */
  readonly endUsers: EndUsersNamespace;
  /** Spend reports and the attributed request log. */
  readonly spend: SpendNamespace;
  /** Gateway admin: runtime models, provider configs, routes and settings. */
  readonly admin: AdminNamespace;

  constructor(opts: LiteGenClientOptions = {}) {
    this.baseUrl = (opts.baseUrl ?? "http://localhost:4000").replace(/\/$/, "");
    this.apiKey = opts.apiKey;
    this.getAuthToken = opts.getAuthToken;
    this.credentials = opts.credentials ?? "same-origin";
    this.getCsrfToken = opts.getCsrfToken;
    this.onError = opts.onError;
    this.fetchImpl = opts.fetch ?? globalThis.fetch.bind(globalThis);
    this.timeoutMs = opts.timeoutMs ?? 60_000;
    this.defaultHeaders = opts.defaultHeaders ?? {};

    this.images = new ImagesNamespace(this);
    this.videos = new VideosNamespace(this);
    this.models3d = new Models3dNamespace(this);
    this.models = new ModelsNamespace(this);
    this.providers = new ProvidersNamespace(this);
    this.health = new HealthNamespace(this);
    this.stats = new StatsNamespace(this);
    this.logs = new LogsNamespace(this);
    this.keys = new KeysNamespace(this);
    this.cache = new CacheNamespace(this);
    this.auth = new AuthNamespace(this);
    this.users = new UsersNamespace(this);
    this.account = new AccountNamespace(this);
    this.audit = new AuditNamespace(this);
    this.generations = new GenerationsNamespace(this);
    this.orgs = new OrgsNamespace(this);
    this.apps = new AppsNamespace(this);
    this.limits = new LimitsNamespace(this);
    this.endUsers = new EndUsersNamespace(this);
    this.spend = new SpendNamespace(this);
    this.admin = new AdminNamespace(this);

    this.activeOrgId = opts.activeOrgId;
    this.activeAppId = opts.activeAppId;
  }

  /**
   * Set (or clear) the active organization / application. When set, every
   * request includes `X-Litegen-Org-Id` and/or `X-Litegen-App-Id` headers so
   * the backend resolves the caller's active tenant context. Pass `undefined`
   * for either argument to clear it.
   */
  setActiveTenant(orgId?: string, appId?: string): void {
    this.activeOrgId = orgId;
    this.activeAppId = appId;
  }

  /** The currently active org id, if any. */
  getActiveOrgId(): string | undefined {
    return this.activeOrgId;
  }

  /** The currently active app id, if any. */
  getActiveAppId(): string | undefined {
    return this.activeAppId;
  }

  /**
   * Build the headers sent on every request — Bearer token, active-tenant
   * context (`X-Litegen-Org-Id` / `X-Litegen-App-Id`), and (for mutating
   * methods) the CSRF token. Shared by {@link request} and {@link requestRaw}
   * so non-JSON downloads (e.g. CSV export) are scoped to the same tenant.
   */
  private async buildHeaders(
    method: string,
    extra?: Record<string, string>,
  ): Promise<Record<string, string>> {
    const headers: Record<string, string> = {
      "Content-Type": "application/json",
      ...this.defaultHeaders,
      ...extra,
    };

    // Bearer token: static apiKey takes precedence, then dynamic getter.
    const token = this.apiKey ?? this.getAuthToken?.();
    if (token) headers["Authorization"] = `Bearer ${token}`;

    // Active-tenant context headers (set via setActiveTenant / constructor opts).
    if (this.activeOrgId) headers["X-Litegen-Org-Id"] = this.activeOrgId;
    if (this.activeAppId) headers["X-Litegen-App-Id"] = this.activeAppId;

    // CSRF token for mutating requests.
    const isMutating = !["GET", "HEAD", "OPTIONS"].includes(method.toUpperCase());
    if (isMutating && this.getCsrfToken) {
      const csrf = await this.getCsrfToken();
      if (csrf) headers["X-CSRF-Token"] = csrf;
    }

    return headers;
  }

  /**
   * Execute a fetch with the shared auth/tenant/CSRF headers, timeout, and
   * abort wiring. Returns the raw {@link Response} (no JSON parsing) and throws
   * {@link LiteGenAPIError} on non-2xx. Used by {@link request} and by raw
   * binary/text downloads such as CSV export.
   * @internal
   */
  async fetchOk(
    method: string,
    path: string,
    body?: unknown,
    signal?: AbortSignal,
    extraHeaders?: Record<string, string>,
  ): Promise<Response> {
    const url = `${this.baseUrl}${path}`;
    const headers = await this.buildHeaders(method, extraHeaders);

    const ctrl = new AbortController();
    const timeout = setTimeout(() => ctrl.abort(), this.timeoutMs);
    if (signal) {
      signal.addEventListener("abort", () => ctrl.abort(), { once: true });
    }

    let res: Response;
    try {
      res = await this.fetchImpl(url, {
        method,
        headers,
        // Bytes (a file to convert) go as they are; everything else is JSON.
        // The cast is for TS >= 5.7, whose BodyInit only admits views over a
        // plain ArrayBuffer; spelling that generically in RawBody would break
        // consumers compiling our .d.ts with an older TypeScript.
        body: body === undefined ? undefined : isRawBody(body) ? (body as BodyInit) : JSON.stringify(body),
        credentials: this.credentials,
        signal: ctrl.signal,
      });
    } catch (err) {
      if ((err as { name?: string }).name === "AbortError" && !signal?.aborted) {
        throw new LiteGenTimeoutError();
      }
      throw err;
    } finally {
      clearTimeout(timeout);
    }

    if (!res.ok) {
      let detail: Schemas["ErrorDetail"] = {
        message: res.statusText,
        type: "http_error",
        code: String(res.status),
      };
      let parsedBody: unknown = null;
      try {
        parsedBody = (await res.json()) as unknown;
        const casted = parsedBody as { error?: Schemas["ErrorDetail"] };
        if (casted?.error) detail = casted.error;
      } catch {
        // Non-JSON error body; keep the default detail.
      }
      this.onError?.(res.status, parsedBody);
      throw new LiteGenAPIError(res.status, detail);
    }
    return res;
  }

  /** @internal */
  async request<T>(
    method: string,
    path: string,
    body?: unknown,
    signal?: AbortSignal,
  ): Promise<T> {
    return (await this.requestWithHeaders<T>(method, path, body, signal)).body;
  }

  /**
   * Like {@link request}, but also hands back the response headers — for the
   * endpoints whose contract includes one, such as the generation POSTs'
   * `X-Litegen-Dropped-Params`.
   * @internal
   */
  async requestWithHeaders<T>(
    method: string,
    path: string,
    body?: unknown,
    signal?: AbortSignal,
  ): Promise<{ body: T; headers: Headers }> {
    const res = await this.fetchOk(method, path, body, signal);
    // 204 No Content and similar empty responses — return undefined cast to T.
    if (res.status === 204 || res.headers.get("content-length") === "0") {
      return { body: undefined as unknown as T, headers: res.headers };
    }
    return { body: (await res.json()) as T, headers: res.headers };
  }

  /**
   * Like {@link request} but returns the response body as a {@link Blob} —
   * for non-JSON downloads such as CSV export. Reuses the same auth +
   * active-tenant headers, so the download is scoped to the caller's active
   * org/app rather than the server-side first-org fallback.
   * @internal
   */
  async requestBlob(
    method: string,
    path: string,
    signal?: AbortSignal,
  ): Promise<Blob> {
    const res = await this.fetchOk(method, path, undefined, signal);
    return await res.blob();
  }
}

class ImagesNamespace {
  constructor(private readonly client: LiteGenClient) {}
  generate(req: ImageRequest, signal?: AbortSignal): Promise<ImageResponse> {
    return this.client.request("POST", "/v1/images/generations", req, signal);
  }
  estimateCost(req: ImageRequest, signal?: AbortSignal): Promise<CostEstimate> {
    return this.client.request("POST", "/v1/images/cost", req, signal);
  }
}

/** What a generation POST returned: its body, plus anything lax mode dropped. */
export interface JobSubmission<T> {
  body: T;
  /** Parsed `X-Litegen-Dropped-Params`; empty when nothing was dropped. */
  droppedParams: string[];
}

/**
 * The shape every async generation handle shares: submit once, then poll the
 * job to a terminal status. {@link VideoJob} and {@link Model3dJob} are this
 * class under their own response type rather than copies of each other, so a
 * fix to how a job submits or polls lands in both.
 *
 * Awaitable (resolves to the final completed / failed / cancelled job) and
 * async-iterable (yields every poll, including the terminal one). There's no
 * SSE on the wire — it submits then polls under the hood. Submission is lazy
 * and memoized: the POST fires on first use, exactly once.
 */
export class GenerationJob<T extends { id: string }> implements PromiseLike<T>, AsyncIterable<T> {
  private submission?: Promise<JobSubmission<T>>;
  private submittedBody?: Promise<T>;

  /**
   * @internal Jobs come from `client.videos.generate` / `client.models3d.generate`;
   * constructing one directly is not part of the public API.
   */
  constructor(
    private readonly submitFn: () => Promise<JobSubmission<T>>,
    private readonly pollFn: (id: string) => AsyncGenerator<T, T, void>,
  ) {}

  private submit(): Promise<JobSubmission<T>> {
    if (!this.submission) this.submission = this.submitFn();
    return this.submission;
  }

  /** The initial submit response (id, status) — does not wait for completion. */
  get submitted(): Promise<T> {
    if (!this.submittedBody) this.submittedBody = this.submit().then((s) => s.body);
    return this.submittedBody;
  }

  /**
   * Params the server dropped rather than rejected, because the request set
   * `strict: false` and the model does not support them — e.g. `["rig",
   * "reference_images[view-top]"]`. Always empty under `strict: true`, where
   * an unsupported param is a 400 instead. Resolves once the job is submitted
   * (submitting it if nothing else has yet).
   */
  get droppedParams(): Promise<string[]> {
    return this.submit().then((s) => s.droppedParams);
  }

  then<TResult1 = T, TResult2 = never>(
    onfulfilled?: ((value: T) => TResult1 | PromiseLike<TResult1>) | null,
    onrejected?: ((reason: unknown) => TResult2 | PromiseLike<TResult2>) | null,
  ): Promise<TResult1 | TResult2> {
    return this.runToCompletion().then(onfulfilled, onrejected);
  }

  async *[Symbol.asyncIterator](): AsyncGenerator<T, void, void> {
    const job = await this.submitted;
    yield* this.pollFn(job.id);
  }

  private async runToCompletion(): Promise<T> {
    const job = await this.submitted;
    let last: T = job;
    for await (const update of this.pollFn(job.id)) last = update;
    return last;
  }
}

/** A generation POST's body, with its dropped-params header parsed alongside. */
async function withDroppedParams<T>(
  response: Promise<{ body: T; headers: Headers }>,
): Promise<JobSubmission<T>> {
  const { body, headers } = await response;
  return { body, droppedParams: parseDroppedParams(headers.get(DROPPED_PARAMS_HEADER)) };
}

/**
 * Handle returned by `videos.generate`. Video generation is async, so this is
 * both awaitable and async-iterable:
 *
 * ```ts
 * // await → the final completed / failed / cancelled job
 * const video = await client.videos.generate({ ... });
 * console.log(video.video_url);
 *
 * // for await → stream progress (each update carries the unified 0–100 progress)
 * for await (const update of client.videos.generate({ ... })) {
 *   console.log(`${update.status} — ${update.progress}%`);
 * }
 * ```
 *
 * Use `await job.submitted` if you only want the initial job id without
 * waiting, and `await job.droppedParams` to see what lax mode dropped.
 */
export class VideoJob extends GenerationJob<VideoResponse> {}

class VideosNamespace {
  constructor(private readonly client: LiteGenClient) {}
  /**
   * Submit a video job (`POST /v1/videos/generations`). The returned
   * {@link VideoJob} is awaitable (resolves to the finished job) and
   * async-iterable (streams progress).
   */
  generate(req: VideoRequest, opts?: WaitForCompletionOptions): VideoJob {
    return new VideoJob(
      () =>
        withDroppedParams(
          this.client.requestWithHeaders<VideoResponse>(
            "POST",
            "/v1/videos/generations",
            req,
            opts?.signal,
          ),
        ),
      (id) => pollVideo(id, (vid) => this.getStatus(vid, opts?.signal), opts),
    );
  }
  estimateCost(req: VideoRequest, signal?: AbortSignal): Promise<CostEstimate> {
    return this.client.request("POST", "/v1/videos/cost", req, signal);
  }
  getStatus(id: string, signal?: AbortSignal): Promise<VideoResponse> {
    return this.client.request("GET", `/v1/videos/${encodeURIComponent(id)}`, undefined, signal);
  }
  waitForCompletion(id: string, opts?: WaitForCompletionOptions): Promise<VideoResponse> {
    return waitForCompletion(id, (videoId) => this.getStatus(videoId, opts?.signal), opts);
  }
  /**
   * Stream progress updates for a video job as an async iterable:
   * `for await (const update of client.videos.poll(job.id)) { ... }`.
   * Yields each poll (incl. the terminal one); `update.progress` is the unified
   * 0–100 value.
   */
  poll(
    id: string,
    opts?: WaitForCompletionOptions,
  ): AsyncGenerator<VideoResponse, VideoResponse, void> {
    return pollVideo(id, (videoId) => this.getStatus(videoId, opts?.signal), opts);
  }
}

/**
 * Handle returned by `models3d.generate`. 3D generation is always async, so
 * this is both awaitable and async-iterable — the same shape as {@link VideoJob}:
 *
 * ```ts
 * const job = await client.models3d.generate({ model, prompt, output_formats: ["glb", "obj"] });
 * if (job.status === "completed") {
 *   const glb = job.assets?.find((a) => a.kind === "mesh" && a.format === "glb");
 * }
 *
 * for await (const update of client.models3d.generate({ ... })) {
 *   console.log(`${update.status} — ${update.progress}%`);
 * }
 * ```
 *
 * `output_formats` is a promise, not a hint: a `completed` job carries a
 * `kind: "mesh"` asset for every format it asked for (plus any texture and
 * preview files), each with an absolute URL. A job that could not deliver one
 * ends `failed` with `error` set — there is no partial success to check for.
 * Polling defaults to {@link MODEL3D_DEFAULT_TIMEOUT_MS}.
 */
export class Model3dJob extends GenerationJob<Model3dResponse> {}

/** `client.models3d` — the 3D family (`/v1/models3d/*`). */
class Models3dNamespace {
  constructor(private readonly client: LiteGenClient) {}
  /**
   * Submit a 3D job (`POST /v1/models3d/generations`). The returned
   * {@link Model3dJob} is awaitable (resolves to the finished job) and
   * async-iterable (streams progress). Validation failures — an unsupported
   * format, too many formats for the model, a missing required reference
   * image — reject before the provider is called, so they cost nothing.
   */
  generate(req: Model3dRequest, opts?: WaitForCompletionOptions): Model3dJob {
    return new Model3dJob(
      () =>
        withDroppedParams(
          this.client.requestWithHeaders<Model3dResponse>(
            "POST",
            "/v1/models3d/generations",
            req,
            opts?.signal,
          ),
        ),
      (id) => poll3d(id, (jid) => this.getStatus(jid, opts?.signal), opts),
    );
  }
  /** Quote a 3D job without running it (`POST /v1/models3d/cost`). */
  estimateCost(req: Model3dRequest, signal?: AbortSignal): Promise<CostEstimate> {
    return this.client.request("POST", "/v1/models3d/cost", req, signal);
  }
  /** One poll of a 3D job (`GET /v1/models3d/{id}`). */
  getStatus(id: string, signal?: AbortSignal): Promise<Model3dResponse> {
    return this.client.request("GET", `/v1/models3d/${encodeURIComponent(id)}`, undefined, signal);
  }
  /** Poll a submitted 3D job until it is completed, failed or cancelled. */
  waitForCompletion(id: string, opts?: WaitForCompletionOptions): Promise<Model3dResponse> {
    return waitForModel3d(id, (jid) => this.getStatus(jid, opts?.signal), opts);
  }
  /**
   * Stream progress updates for a 3D job as an async iterable:
   * `for await (const update of client.models3d.poll(job.id)) { ... }`.
   * Yields each poll (incl. the terminal one); `update.progress` is the unified
   * 0–100 value. Cancel a job with `client.generations.cancel(id)`.
   */
  poll(
    id: string,
    opts?: WaitForCompletionOptions,
  ): AsyncGenerator<Model3dResponse, Model3dResponse, void> {
    return poll3d(id, (jid) => this.getStatus(jid, opts?.signal), opts);
  }
  /**
   * Convert a mesh file you already have to another container
   * (`POST /v1/utils/models3d/convert`). A standalone utility: no provider,
   * no generation, nothing billed.
   *
   * ```ts
   * const { blob, filename } = await client.models3d.convert(glbBytes, { from: "glb", to: "stl" });
   * ```
   *
   * Rejects with `LiteGenAPIError` on a pair the converter does not support
   * (400 `format_unsupported`, `param` says which), a file that is not a `from`
   * file (400 `format_mismatch`), one over the size cap (413), or a mesh it
   * cannot read or convert (422 `mesh_malformed` / `mesh_unsupported` /
   * `mesh_empty`).
   */
  async convert(input: RawBody, opts: ConvertModel3dOptions): Promise<Model3dConversion> {
    const res = await this.client.fetchOk(
      "POST",
      `/v1/utils/models3d/convert?from=${encodeURIComponent(opts.from)}&to=${encodeURIComponent(opts.to)}`,
      input,
      opts.signal,
      { "Content-Type": "application/octet-stream" },
    );
    return {
      blob: await res.blob(),
      format: opts.to,
      filename: dispositionFilename(res.headers.get("content-disposition")) ?? `model.${opts.to}`,
      // Same comma-joined shape as the dropped-params header, so the same parser.
      losses: parseDroppedParams(res.headers.get(CONVERSION_LOSSES_HEADER)),
    };
  }
  /**
   * What the converter accepts (`GET /v1/utils/models3d/formats`): the
   * containers usable as both `from` and `to` — any pair converts — and the
   * largest input it takes. Narrower than `Model3dFormat`, and it grows with
   * litegen's converter, so read it rather than hard-coding it.
   */
  convertFormats(signal?: AbortSignal): Promise<Schemas["Model3dConvertFormats"]> {
    return this.client.request("GET", "/v1/utils/models3d/formats", undefined, signal);
  }
}

/** True for an AbortSignal from any realm (polyfilled fetch brings its own). */
function isAbortSignal(v: unknown): v is AbortSignal {
  return (
    typeof v === "object" &&
    v !== null &&
    typeof (v as AbortSignal).aborted === "boolean" &&
    typeof (v as AbortSignal).addEventListener === "function"
  );
}

class ModelsNamespace {
  constructor(private readonly client: LiteGenClient) {}
  /**
   * The model catalog (`GET /v1/models`), optionally narrowed to one family:
   * `models.list({ media_type: "model3d" })`. `models.list(signal)` — the
   * original signature — still works.
   */
  async list(
    opts?: ListModelsOptions | AbortSignal,
    signal?: AbortSignal,
  ): Promise<ModelInfo[]> {
    const options: ListModelsOptions = isAbortSignal(opts) ? {} : (opts ?? {});
    const sig = isAbortSignal(opts) ? opts : signal;
    const resp = options.media_type
      ? await this.client.request<ModelListResponse>(
          "GET",
          `/v1/models?media_type=${encodeURIComponent(options.media_type)}`,
          undefined,
          sig,
        )
      : await this.client.request<ModelListResponse>("GET", "/v1/models", undefined, sig);
    return resp.data;
  }
  /**
   * One model's full capability schema (`GET /v1/models/{id}`): its params
   * with their allowed values, prompt rules, reference-image roles and
   * provider passthrough keys — everything needed to build a valid request.
   */
  get(id: string, signal?: AbortSignal): Promise<ModelSchema> {
    return this.client.request("GET", `/v1/models/${encodeURIComponent(id)}`, undefined, signal);
  }
  /** Alias kept for compatibility with older callers. */
  getSchema(id: string, signal?: AbortSignal): Promise<ModelSchema> {
    return this.get(id, signal);
  }
}

class ProvidersNamespace {
  constructor(private readonly client: LiteGenClient) {}
  /** Catalog of providers and the credential fields each one needs. Drives the
   *  dashboard's dynamic credential form so it never hard-codes per-provider knowledge. */
  list(signal?: AbortSignal): Promise<ProviderCatalogEntry[]> {
    return this.client.request("GET", "/v1/providers", undefined, signal);
  }
}

class HealthNamespace {
  constructor(private readonly client: LiteGenClient) {}
  check(signal?: AbortSignal): Promise<HealthResponse> {
    return this.client.request("GET", "/health", undefined, signal);
  }
  /** Alias used by Health page. */
  get(signal?: AbortSignal): Promise<HealthResponse> {
    return this.check(signal);
  }
  live(signal?: AbortSignal): Promise<LivenessResponse> {
    return this.client.request("GET", "/health/live", undefined, signal);
  }
  ready(signal?: AbortSignal): Promise<{ status: string }> {
    return this.client.request("GET", "/health/ready", undefined, signal);
  }
}

class StatsNamespace {
  constructor(private readonly client: LiteGenClient) {}
  get(signal?: AbortSignal): Promise<ProxyStats> {
    return this.client.request("GET", "/v1/stats", undefined, signal);
  }
}

/** Append the shared request-log filters (model/provider/status/from/to) to a query string. */
function applyLogFilters(params: URLSearchParams, opts: ExportLogsOptions): void {
  if (opts.model) params.set("model", opts.model);
  if (opts.provider) params.set("provider", opts.provider);
  if (opts.status) params.set("status", opts.status);
  if (opts.from) params.set("from", opts.from);
  if (opts.to) params.set("to", opts.to);
}

/** Append the shared audit-log filters (actor_key_id/action/from/to) to a query string. */
function applyAuditFilters(params: URLSearchParams, opts: ExportAuditLogOptions): void {
  if (opts.actor_key_id) params.set("actor_key_id", opts.actor_key_id);
  if (opts.action) params.set("action", opts.action);
  if (opts.from) params.set("from", opts.from);
  if (opts.to) params.set("to", opts.to);
}

class LogsNamespace {
  constructor(private readonly client: LiteGenClient) {}
  list(
    opts: GetLogsOptions = {},
    signal?: AbortSignal,
  ): Promise<PaginatedResponse<RequestLog>> {
    const { page = 1, per_page = 50 } = opts;
    const params = new URLSearchParams({ page: String(page), per_page: String(per_page) });
    applyLogFilters(params, opts);
    return this.client.request(
      "GET",
      `/v1/logs?${params.toString()}`,
      undefined,
      signal,
    );
  }
  /**
   * Download the request logs as CSV for the given filters. Hits the same
   * `/v1/logs` endpoint with `format=csv` and returns the body as a {@link Blob}.
   * Reuses the client's auth + active-tenant headers, so the export is scoped
   * to the caller's active org/app (not the server-side first-org fallback).
   */
  exportCsv(opts: ExportLogsOptions = {}, signal?: AbortSignal): Promise<Blob> {
    const params = new URLSearchParams({ format: "csv" });
    applyLogFilters(params, opts);
    return this.client.requestBlob("GET", `/v1/logs?${params.toString()}`, signal);
  }
  getArtifact(id: string, signal?: AbortSignal): Promise<RequestArtifact> {
    return this.client.request(
      "GET",
      `/v1/logs/${encodeURIComponent(id)}/artifact`,
      undefined,
      signal,
    );
  }
}

class KeyLimitsNamespace {
  constructor(private readonly client: LiteGenClient) {}
  get(keyId: string, signal?: AbortSignal): Promise<EntityLimits> {
    return this.client.request("GET", `/v1/keys/${encodeURIComponent(keyId)}/limits`, undefined, signal);
  }
  /** Replace the key's budget window, soft budget, model allowlist, rpm and parallel caps. */
  set(keyId: string, limits: LimitsInput, signal?: AbortSignal): Promise<EntityLimits> {
    return this.client.request("PUT", `/v1/keys/${encodeURIComponent(keyId)}/limits`, limits, signal);
  }
  delete(keyId: string, signal?: AbortSignal): Promise<DeletedResponse> {
    return this.client.request("DELETE", `/v1/keys/${encodeURIComponent(keyId)}/limits`, undefined, signal);
  }
  /** Zero the key's spend for the current window. */
  resetSpend(keyId: string, signal?: AbortSignal): Promise<EntityLimits> {
    return this.client.request("POST", `/v1/keys/${encodeURIComponent(keyId)}/limits/reset`, undefined, signal);
  }
}

class KeysNamespace {
  readonly limits: KeyLimitsNamespace;
  constructor(private readonly client: LiteGenClient) {
    this.limits = new KeyLimitsNamespace(client);
  }
  create(body: CreateKeyBody | string, signal?: AbortSignal): Promise<ApiKeyCreatedResponse> {
    // Accept either a full body object or just a name string for back-compat.
    const reqBody = typeof body === "string" ? { name: body } : body;
    return this.client.request("POST", "/v1/keys", reqBody, signal);
  }
  async list(signal?: AbortSignal): Promise<ApiKeyInfo[]> {
    const resp = await this.client.request<ApiKeyListResponse>(
      "GET",
      "/v1/keys",
      undefined,
      signal,
    );
    return resp.data;
  }
  /** One key's detail (`GET /v1/keys/{id}`). */
  get(id: string, signal?: AbortSignal): Promise<ApiKeyDetail> {
    return this.client.request("GET", `/v1/keys/${encodeURIComponent(id)}`, undefined, signal);
  }
  /** Refuse every request made with this key until {@link unblock}ed. */
  block(id: string, signal?: AbortSignal): Promise<KeyActiveResponse> {
    return this.client.request("POST", `/v1/keys/${encodeURIComponent(id)}/block`, undefined, signal);
  }
  unblock(id: string, signal?: AbortSignal): Promise<KeyActiveResponse> {
    return this.client.request("POST", `/v1/keys/${encodeURIComponent(id)}/unblock`, undefined, signal);
  }
  patch(id: string, body: PatchKeyBody, signal?: AbortSignal): Promise<ApiKeyDetail> {
    // Server returns ApiKeyDetail (key_prefix; no public_id/app_id) — not ApiKeyInfo.
    return this.client.request("PATCH", `/v1/keys/${encodeURIComponent(id)}`, body, signal);
  }
  revoke(id: string, signal?: AbortSignal): Promise<RevokeKeyResponse> {
    return this.client.request("DELETE", `/v1/keys/${encodeURIComponent(id)}`, undefined, signal);
  }
  /** Alias: plan calls `client.keys.delete` */
  delete(id: string, signal?: AbortSignal): Promise<RevokeKeyResponse> {
    return this.revoke(id, signal);
  }
  rotate(id: string, signal?: AbortSignal): Promise<RotatedKeyResponse> {
    // Server returns RotatedKeyResponse (no created_at) — not ApiKeyCreatedResponse.
    return this.client.request("POST", `/v1/keys/${encodeURIComponent(id)}/rotate`, undefined, signal);
  }
  testWebhook(id: string, signal?: AbortSignal): Promise<TestWebhookResponse> {
    return this.client.request("POST", `/v1/keys/${encodeURIComponent(id)}/test-webhook`, undefined, signal);
  }
  listWebhookDeliveries(
    id: string,
    opts: ListWebhookDeliveriesOptions = {},
    signal?: AbortSignal,
  ): Promise<PaginatedResponse<WebhookDelivery>> {
    const { page = 1, per_page = 20 } = opts;
    return this.client.request(
      "GET",
      `/v1/keys/${encodeURIComponent(id)}/webhook-deliveries?page=${page}&per_page=${per_page}`,
      undefined,
      signal,
    );
  }
}

class CacheNamespace {
  constructor(private readonly client: LiteGenClient) {}
  clear(signal?: AbortSignal): Promise<CacheClearedResponse> {
    return this.client.request("DELETE", "/v1/cache", undefined, signal);
  }
}

class AuthNamespace {
  constructor(private readonly client: LiteGenClient) {}

  /** GET /v1/auth/config — public; reports which auth methods are enabled. */
  config(signal?: AbortSignal): Promise<AuthConfigResponse> {
    return this.client.request("GET", "/v1/auth/config", undefined, signal);
  }
  signup(req: SignupRequest, signal?: AbortSignal): Promise<AuthResponse> {
    return this.client.request("POST", "/v1/auth/signup", req, signal);
  }
  login(req: LoginRequest, signal?: AbortSignal): Promise<AuthResponse> {
    return this.client.request("POST", "/v1/auth/login", req, signal);
  }
  logout(signal?: AbortSignal): Promise<void> {
    return this.client.request("POST", "/v1/auth/logout", undefined, signal);
  }
  me(signal?: AbortSignal): Promise<unknown> {
    return this.client.request("GET", "/v1/auth/me", undefined, signal);
  }
  csrf(signal?: AbortSignal): Promise<CsrfResponse> {
    return this.client.request("GET", "/v1/auth/csrf", undefined, signal);
  }
  passwordResetRequest(req: PasswordResetRequestBody, signal?: AbortSignal): Promise<unknown> {
    return this.client.request("POST", "/v1/auth/password-reset/request", req, signal);
  }
  passwordResetConfirm(req: PasswordResetConfirmBody, signal?: AbortSignal): Promise<void> {
    return this.client.request("POST", "/v1/auth/password-reset/confirm", req, signal);
  }
  oauthGithubStart(signal?: AbortSignal): Promise<void> {
    return this.client.request("GET", "/v1/auth/oauth/github/start", undefined, signal);
  }
  oauthGithubCallback(code: string, state: string, signal?: AbortSignal): Promise<void> {
    return this.client.request(
      "GET",
      `/v1/auth/oauth/github/callback?code=${encodeURIComponent(code)}&state=${encodeURIComponent(state)}`,
      undefined,
      signal,
    );
  }
  oauthGoogleStart(signal?: AbortSignal): Promise<void> {
    return this.client.request("GET", "/v1/auth/oauth/google/start", undefined, signal);
  }
  oauthGoogleCallback(code: string, state: string, signal?: AbortSignal): Promise<void> {
    return this.client.request(
      "GET",
      `/v1/auth/oauth/google/callback?code=${encodeURIComponent(code)}&state=${encodeURIComponent(state)}`,
      undefined,
      signal,
    );
  }
  getInvitation(token: string, signal?: AbortSignal): Promise<InvitationView> {
    return this.client.request(
      "GET",
      `/v1/auth/invitations/${encodeURIComponent(token)}`,
      undefined,
      signal,
    );
  }
  acceptInvitation(token: string, req: AcceptInvitationRequest, signal?: AbortSignal): Promise<AuthResponse> {
    return this.client.request(
      "POST",
      `/v1/auth/invitations/${encodeURIComponent(token)}/accept`,
      req,
      signal,
    );
  }
}

class UsersNamespace {
  constructor(private readonly client: LiteGenClient) {}

  list(signal?: AbortSignal): Promise<PublicUser[]> {
    return this.client.request("GET", "/v1/users", undefined, signal);
  }
  invite(req: InviteRequest, signal?: AbortSignal): Promise<InviteResponse> {
    return this.client.request("POST", "/v1/users", req, signal);
  }
  patch(id: string, req: PatchUserRequest, signal?: AbortSignal): Promise<PublicUser> {
    return this.client.request("PATCH", `/v1/users/${encodeURIComponent(id)}`, req, signal);
  }
  delete(id: string, signal?: AbortSignal): Promise<void> {
    return this.client.request("DELETE", `/v1/users/${encodeURIComponent(id)}`, undefined, signal);
  }
  transferOwner(req: TransferOwnerRequest, signal?: AbortSignal): Promise<void> {
    return this.client.request("POST", "/v1/users/transfer-owner", req, signal);
  }
}

class AccountNamespace {
  constructor(private readonly client: LiteGenClient) {}

  get(signal?: AbortSignal): Promise<AccountUser> {
    return this.client.request("GET", "/v1/account", undefined, signal);
  }
  patch(req: PatchAccountRequest, signal?: AbortSignal): Promise<AccountUser> {
    return this.client.request("PATCH", "/v1/account", req, signal);
  }
  listSessions(signal?: AbortSignal): Promise<SessionInfo[]> {
    return this.client.request("GET", "/v1/account/sessions", undefined, signal);
  }
  revokeSession(id: string, signal?: AbortSignal): Promise<void> {
    return this.client.request(
      "DELETE",
      `/v1/account/sessions/${encodeURIComponent(id)}`,
      undefined,
      signal,
    );
  }
}

class AuditNamespace {
  constructor(private readonly client: LiteGenClient) {}

  list(opts: ListAuditLogOptions = {}, signal?: AbortSignal): Promise<PaginatedResponse<AuditLogEntry>> {
    const { page = 1, per_page = 50 } = opts;
    const params = new URLSearchParams({ page: String(page), per_page: String(per_page) });
    applyAuditFilters(params, opts);
    return this.client.request(
      "GET",
      `/v1/audit?${params.toString()}`,
      undefined,
      signal,
    );
  }
  /**
   * Download the audit log as CSV for the given filters. Hits the same
   * `/v1/audit` endpoint with `format=csv` and returns the body as a {@link Blob}.
   * Reuses the client's auth + active-tenant headers, so the export is scoped
   * to the caller's active org/app (not the server-side first-org fallback).
   */
  exportCsv(opts: ExportAuditLogOptions = {}, signal?: AbortSignal): Promise<Blob> {
    const params = new URLSearchParams({ format: "csv" });
    applyAuditFilters(params, opts);
    return this.client.requestBlob("GET", `/v1/audit?${params.toString()}`, signal);
  }
}

class GenerationsNamespace {
  constructor(private readonly client: LiteGenClient) {}

  list(
    opts: { page?: number; per_page?: number } = {},
    signal?: AbortSignal,
  ): Promise<PaginatedResponse<Generation>> {
    const { page = 1, per_page = 25 } = opts;
    return this.client.request(
      "GET",
      `/v1/generations?page=${page}&per_page=${per_page}`,
      undefined,
      signal,
    );
  }

  /**
   * Fetch one generation row by id. Rejects with `LiteGenAPIError` (status 404)
   * when the row does not exist — which, just after submitting an async job,
   * can simply mean the row has not been persisted yet.
   */
  get(id: string, signal?: AbortSignal): Promise<Generation> {
    return this.client.request(
      "GET",
      `/v1/generations/${encodeURIComponent(id)}`,
      undefined,
      signal,
    );
  }

  /**
   * Cancel an in-flight async job — video or 3D — by id
   * (`PATCH /v1/generations/{id}` with `{ status: "cancelled" }`). Rejects
   * with `LiteGenAPIError` status 409 once the job is no longer pending or
   * processing, and 404 for an unknown id.
   */
  cancel(id: string, signal?: AbortSignal): Promise<Generation> {
    return this.client.request(
      "PATCH",
      `/v1/generations/${encodeURIComponent(id)}`,
      { status: "cancelled" },
      signal,
    );
  }
}

// ─── Tenancy namespaces ────────────────────────────────────────────────────────

class OrgMembersNamespace {
  constructor(private readonly client: LiteGenClient) {}

  list(orgId: string, signal?: AbortSignal): Promise<MemberView[]> {
    return this.client.request(
      "GET",
      `/v1/orgs/${encodeURIComponent(orgId)}/members`,
      undefined,
      signal,
    );
  }
  invite(orgId: string, req: AddMemberRequest, signal?: AbortSignal): Promise<unknown> {
    return this.client.request(
      "POST",
      `/v1/orgs/${encodeURIComponent(orgId)}/members`,
      req,
      signal,
    );
  }
  updateRole(
    orgId: string,
    userId: string,
    req: UpdateMemberRequest,
    signal?: AbortSignal,
  ): Promise<unknown> {
    return this.client.request(
      "PATCH",
      `/v1/orgs/${encodeURIComponent(orgId)}/members/${encodeURIComponent(userId)}`,
      req,
      signal,
    );
  }
  remove(orgId: string, userId: string, signal?: AbortSignal): Promise<void> {
    return this.client.request(
      "DELETE",
      `/v1/orgs/${encodeURIComponent(orgId)}/members/${encodeURIComponent(userId)}`,
      undefined,
      signal,
    );
  }
}

class OrgProviderCredentialsNamespace {
  constructor(private readonly client: LiteGenClient) {}

  list(orgId: string, signal?: AbortSignal): Promise<ProviderCredentialInfo[]> {
    return this.client.request("GET", `/v1/orgs/${encodeURIComponent(orgId)}/provider-credentials`, undefined, signal);
  }
  create(orgId: string, req: CreateProviderCredentialRequest, signal?: AbortSignal): Promise<ProviderCredentialInfo> {
    return this.client.request("POST", `/v1/orgs/${encodeURIComponent(orgId)}/provider-credentials`, req, signal);
  }
  delete(orgId: string, provider: string, signal?: AbortSignal): Promise<void> {
    return this.client.request("DELETE", `/v1/orgs/${encodeURIComponent(orgId)}/provider-credentials/${encodeURIComponent(provider)}`, undefined, signal);
  }
}

class OrgAllowedModelsNamespace {
  constructor(private readonly client: LiteGenClient) {}

  get(orgId: string, signal?: AbortSignal): Promise<OrgAllowedModels> {
    return this.client.request(
      'GET',
      `/v1/orgs/${encodeURIComponent(orgId)}/allowed-models`,
      undefined,
      signal,
    );
  }

  set(orgId: string, req: SetOrgAllowedModelsRequest, signal?: AbortSignal): Promise<OrgAllowedModels> {
    return this.client.request(
      'PUT',
      `/v1/orgs/${encodeURIComponent(orgId)}/allowed-models`,
      req,
      signal,
    );
  }
}

class OrgAppsNamespace {
  constructor(private readonly client: LiteGenClient) {}

  list(orgId: string, signal?: AbortSignal): Promise<Application[]> {
    return this.client.request(
      "GET",
      `/v1/orgs/${encodeURIComponent(orgId)}/apps`,
      undefined,
      signal,
    );
  }
  create(orgId: string, req: CreateAppRequest, signal?: AbortSignal): Promise<Application> {
    return this.client.request(
      "POST",
      `/v1/orgs/${encodeURIComponent(orgId)}/apps`,
      req,
      signal,
    );
  }
}

class OrgLimitsNamespace {
  constructor(private readonly client: LiteGenClient) {}
  get(orgId: string, signal?: AbortSignal): Promise<EntityLimits> {
    return this.client.request("GET", `/v1/orgs/${encodeURIComponent(orgId)}/limits`, undefined, signal);
  }
  set(orgId: string, limits: LimitsInput, signal?: AbortSignal): Promise<EntityLimits> {
    return this.client.request("PUT", `/v1/orgs/${encodeURIComponent(orgId)}/limits`, limits, signal);
  }
  delete(orgId: string, signal?: AbortSignal): Promise<DeletedResponse> {
    return this.client.request("DELETE", `/v1/orgs/${encodeURIComponent(orgId)}/limits`, undefined, signal);
  }
  resetSpend(orgId: string, signal?: AbortSignal): Promise<EntityLimits> {
    return this.client.request("POST", `/v1/orgs/${encodeURIComponent(orgId)}/limits/reset`, undefined, signal);
  }
}

class OrgMemberLimitsNamespace {
  constructor(private readonly client: LiteGenClient) {}
  get(orgId: string, userId: string, signal?: AbortSignal): Promise<EntityLimits> {
    return this.client.request("GET", `/v1/orgs/${encodeURIComponent(orgId)}/members/${encodeURIComponent(userId)}/limits`, undefined, signal);
  }
  /** A member's budget inside the org: covers their session requests and every key they own. */
  set(orgId: string, userId: string, limits: LimitsInput, signal?: AbortSignal): Promise<EntityLimits> {
    return this.client.request("PUT", `/v1/orgs/${encodeURIComponent(orgId)}/members/${encodeURIComponent(userId)}/limits`, limits, signal);
  }
  delete(orgId: string, userId: string, signal?: AbortSignal): Promise<DeletedResponse> {
    return this.client.request("DELETE", `/v1/orgs/${encodeURIComponent(orgId)}/members/${encodeURIComponent(userId)}/limits`, undefined, signal);
  }
  resetSpend(orgId: string, userId: string, signal?: AbortSignal): Promise<EntityLimits> {
    return this.client.request("POST", `/v1/orgs/${encodeURIComponent(orgId)}/members/${encodeURIComponent(userId)}/limits/reset`, undefined, signal);
  }
}

class OrgsNamespace {
  readonly members: OrgMembersNamespace;
  readonly apps: OrgAppsNamespace;
  readonly allowedModels: OrgAllowedModelsNamespace;
  readonly providerCredentials: OrgProviderCredentialsNamespace;
  readonly limits: OrgLimitsNamespace;
  readonly memberLimits: OrgMemberLimitsNamespace;

  constructor(private readonly client: LiteGenClient) {
    this.members = new OrgMembersNamespace(client);
    this.apps = new OrgAppsNamespace(client);
    this.allowedModels = new OrgAllowedModelsNamespace(client);
    this.providerCredentials = new OrgProviderCredentialsNamespace(client);
    this.limits = new OrgLimitsNamespace(client);
    this.memberLimits = new OrgMemberLimitsNamespace(client);
  }

  list(signal?: AbortSignal): Promise<OrgSummary[]> {
    return this.client.request("GET", "/v1/orgs", undefined, signal);
  }
  create(req: CreateOrgRequest, signal?: AbortSignal): Promise<OrgView> {
    return this.client.request("POST", "/v1/orgs", req, signal);
  }
  get(id: string, signal?: AbortSignal): Promise<OrgView> {
    return this.client.request("GET", `/v1/orgs/${encodeURIComponent(id)}`, undefined, signal);
  }
  update(id: string, req: UpdateOrgRequest, signal?: AbortSignal): Promise<OrgView> {
    return this.client.request("PATCH", `/v1/orgs/${encodeURIComponent(id)}`, req, signal);
  }
  delete(id: string, signal?: AbortSignal): Promise<void> {
    return this.client.request("DELETE", `/v1/orgs/${encodeURIComponent(id)}`, undefined, signal);
  }
  transferOwner(
    orgId: string,
    req: OrgTransferOwnerRequest,
    signal?: AbortSignal,
  ): Promise<void> {
    return this.client.request(
      "POST",
      `/v1/orgs/${encodeURIComponent(orgId)}/transfer-owner`,
      req,
      signal,
    );
  }
}

class AppStorageNamespace {
  constructor(private readonly client: LiteGenClient) {}

  get(appId: string, signal?: AbortSignal): Promise<AppStorageInfo> {
    return this.client.request(
      "GET",
      `/v1/apps/${encodeURIComponent(appId)}/storage`,
      undefined,
      signal,
    );
  }
  put(
    appId: string,
    req: PutAppStorageRequest,
    signal?: AbortSignal,
  ): Promise<AppStorageInfo> {
    return this.client.request(
      "PUT",
      `/v1/apps/${encodeURIComponent(appId)}/storage`,
      req,
      signal,
    );
  }
  delete(appId: string, signal?: AbortSignal): Promise<void> {
    return this.client.request(
      "DELETE",
      `/v1/apps/${encodeURIComponent(appId)}/storage`,
      undefined,
      signal,
    );
  }
}

class AppModelAccessNamespace {
  constructor(private readonly client: LiteGenClient) {}

  get(appId: string, signal?: AbortSignal): Promise<AppModelAccess> {
    return this.client.request(
      'GET',
      `/v1/apps/${encodeURIComponent(appId)}/allowed-models`,
      undefined,
      signal,
    );
  }

  set(appId: string, req: SetAppModelAccessRequest, signal?: AbortSignal): Promise<AppModelAccess> {
    return this.client.request(
      'PUT',
      `/v1/apps/${encodeURIComponent(appId)}/allowed-models`,
      req,
      signal,
    );
  }
}

class AppLimitsNamespace {
  constructor(private readonly client: LiteGenClient) {}
  get(appId: string, signal?: AbortSignal): Promise<EntityLimits> {
    return this.client.request("GET", `/v1/apps/${encodeURIComponent(appId)}/limits`, undefined, signal);
  }
  set(appId: string, limits: LimitsInput, signal?: AbortSignal): Promise<EntityLimits> {
    return this.client.request("PUT", `/v1/apps/${encodeURIComponent(appId)}/limits`, limits, signal);
  }
  delete(appId: string, signal?: AbortSignal): Promise<DeletedResponse> {
    return this.client.request("DELETE", `/v1/apps/${encodeURIComponent(appId)}/limits`, undefined, signal);
  }
  resetSpend(appId: string, signal?: AbortSignal): Promise<EntityLimits> {
    return this.client.request("POST", `/v1/apps/${encodeURIComponent(appId)}/limits/reset`, undefined, signal);
  }
}

class AppsNamespace {
  readonly storage: AppStorageNamespace;
  readonly allowedModels: AppModelAccessNamespace;
  readonly limits: AppLimitsNamespace;

  constructor(private readonly client: LiteGenClient) {
    this.storage = new AppStorageNamespace(client);
    this.allowedModels = new AppModelAccessNamespace(client);
    this.limits = new AppLimitsNamespace(client);
  }

  get(appId: string, signal?: AbortSignal): Promise<Application> {
    return this.client.request("GET", `/v1/apps/${encodeURIComponent(appId)}`, undefined, signal);
  }
  update(appId: string, req: UpdateAppRequest, signal?: AbortSignal): Promise<Application> {
    return this.client.request("PATCH", `/v1/apps/${encodeURIComponent(appId)}`, req, signal);
  }
  delete(appId: string, signal?: AbortSignal): Promise<void> {
    return this.client.request("DELETE", `/v1/apps/${encodeURIComponent(appId)}`, undefined, signal);
  }
}

// ─── Management: limits, end users, spend, gateway admin ─────────────────────

/** Options for {@link LimitsNamespace.list}. */
export interface ListLimitsOptions {
  entityType?: EntityType;
  /** Platform admin only: another org. */
  orgId?: string;
}

class LimitsNamespace {
  constructor(private readonly client: LiteGenClient) {}
  /** Every limits row in the active org, optionally of one entity type. */
  async list(opts: ListLimitsOptions = {}, signal?: AbortSignal): Promise<EntityLimits[]> {
    const params = new URLSearchParams();
    if (opts.entityType) params.set("entity_type", opts.entityType);
    if (opts.orgId) params.set("org_id", opts.orgId);
    const qs = params.toString();
    const resp = await this.client.request<LimitsListResponse>("GET", `/v1/limits?${qs}`, undefined, signal);
    return resp.data;
  }
  /** The limits that apply to the caller: its key, org, app and member rows. */
  async me(signal?: AbortSignal): Promise<EntityLimits[]> {
    const resp = await this.client.request<LimitsListResponse>("GET", "/v1/limits/me", undefined, signal);
    return resp.data;
  }
}

/** Platform admin only: act on another org's end users. */
export interface EndUserOptions {
  orgId?: string;
}

/** Query string for {@link EndUserOptions}; empty when there is nothing to send. */
function orgQuery(opts: EndUserOptions | undefined): string {
  return opts?.orgId ? new URLSearchParams({ org_id: opts.orgId }).toString() : "";
}

class EndUserLimitsNamespace {
  constructor(private readonly client: LiteGenClient) {}
  get(endUserId: string, opts?: EndUserOptions, signal?: AbortSignal): Promise<EntityLimits> {
    return this.client.request("GET", `/v1/end-users/${encodeURIComponent(endUserId)}/limits?${orgQuery(opts)}`, undefined, signal);
  }
  set(endUserId: string, limits: LimitsInput, opts?: EndUserOptions, signal?: AbortSignal): Promise<EntityLimits> {
    return this.client.request("PUT", `/v1/end-users/${encodeURIComponent(endUserId)}/limits?${orgQuery(opts)}`, limits, signal);
  }
  delete(endUserId: string, opts?: EndUserOptions, signal?: AbortSignal): Promise<DeletedResponse> {
    return this.client.request("DELETE", `/v1/end-users/${encodeURIComponent(endUserId)}/limits?${orgQuery(opts)}`, undefined, signal);
  }
  resetSpend(endUserId: string, opts?: EndUserOptions, signal?: AbortSignal): Promise<EntityLimits> {
    return this.client.request("POST", `/v1/end-users/${encodeURIComponent(endUserId)}/limits/reset?${orgQuery(opts)}`, undefined, signal);
  }
}

class EndUsersNamespace {
  readonly limits: EndUserLimitsNamespace;
  constructor(private readonly client: LiteGenClient) {
    this.limits = new EndUserLimitsNamespace(client);
  }
  async list(opts?: EndUserOptions, signal?: AbortSignal): Promise<EndUserView[]> {
    const resp = await this.client.request<EndUserListResponse>("GET", `/v1/end-users?${orgQuery(opts)}`, undefined, signal);
    return resp.data;
  }
  create(req: CreateEndUserRequest, opts?: EndUserOptions, signal?: AbortSignal): Promise<EndUserView> {
    return this.client.request("POST", `/v1/end-users?${orgQuery(opts)}`, req, signal);
  }
  get(endUserId: string, opts?: EndUserOptions, signal?: AbortSignal): Promise<EndUserView> {
    return this.client.request("GET", `/v1/end-users/${encodeURIComponent(endUserId)}?${orgQuery(opts)}`, undefined, signal);
  }
  update(endUserId: string, req: UpdateEndUserRequest, opts?: EndUserOptions, signal?: AbortSignal): Promise<EndUserView> {
    return this.client.request("PATCH", `/v1/end-users/${encodeURIComponent(endUserId)}?${orgQuery(opts)}`, req, signal);
  }
  delete(endUserId: string, opts?: EndUserOptions, signal?: AbortSignal): Promise<DeletedResponse> {
    return this.client.request("DELETE", `/v1/end-users/${encodeURIComponent(endUserId)}?${orgQuery(opts)}`, undefined, signal);
  }
  /** Refuse this end user's requests (403 `end_user_blocked`). Registers unseen ids. */
  block(endUserId: string, opts?: EndUserOptions, signal?: AbortSignal): Promise<EndUserView> {
    return this.client.request("POST", `/v1/end-users/${encodeURIComponent(endUserId)}/block?${orgQuery(opts)}`, undefined, signal);
  }
  unblock(endUserId: string, opts?: EndUserOptions, signal?: AbortSignal): Promise<EndUserView> {
    return this.client.request("POST", `/v1/end-users/${encodeURIComponent(endUserId)}/unblock?${orgQuery(opts)}`, undefined, signal);
  }
}

/** Filters shared by {@link SpendNamespace.report} and {@link SpendNamespace.logs}. */
export interface SpendFilterOptions {
  /** RFC 3339 timestamp or `YYYY-MM-DD` (inclusive). */
  from?: string;
  /** RFC 3339 timestamp or `YYYY-MM-DD` (inclusive, the whole day). */
  to?: string;
  /** Platform admin only. */
  orgId?: string;
  appId?: string;
  keyId?: string;
  userId?: string;
  endUser?: string;
  model?: string;
  provider?: string;
  tag?: string;
}

export interface SpendReportOptions extends SpendFilterOptions {
  /** Default `day`. */
  groupBy?: SpendGroupBy;
}

export interface SpendLogsOptions extends SpendFilterOptions {
  page?: number;
  perPage?: number;
}

function spendParams(opts: SpendFilterOptions): URLSearchParams {
  const p = new URLSearchParams();
  const set = (k: string, v: string | undefined) => {
    if (v) p.set(k, v);
  };
  set("from", opts.from);
  set("to", opts.to);
  set("org_id", opts.orgId);
  set("app_id", opts.appId);
  set("key_id", opts.keyId);
  set("user_id", opts.userId);
  set("end_user", opts.endUser);
  set("model", opts.model);
  set("provider", opts.provider);
  set("tag", opts.tag);
  return p;
}

class SpendNamespace {
  constructor(private readonly client: LiteGenClient) {}
  /** Spend and request counts grouped by one dimension (day, model, key, end_user, tag, …). */
  report(opts: SpendReportOptions = {}, signal?: AbortSignal): Promise<SpendReportResponse> {
    const params = spendParams(opts);
    if (opts.groupBy) params.set("group_by", opts.groupBy);
    const qs = params.toString();
    return this.client.request("GET", `/v1/spend/report?${qs}`, undefined, signal);
  }
  /** The attributed request log (key, user, end user, tags), newest first. */
  logs(opts: SpendLogsOptions = {}, signal?: AbortSignal): Promise<SpendLogsResponse> {
    const params = spendParams(opts);
    if (opts.page) params.set("page", String(opts.page));
    if (opts.perPage) params.set("per_page", String(opts.perPage));
    const qs = params.toString();
    return this.client.request("GET", `/v1/spend/logs?${qs}`, undefined, signal);
  }
}

class AdminModelsNamespace {
  constructor(private readonly client: LiteGenClient) {}
  /** Every model, catalog and runtime, enabled or not. */
  async list(signal?: AbortSignal): Promise<AdminModelView[]> {
    const resp = await this.client.request<AdminModelListResponse>("GET", "/v1/admin/models", undefined, signal);
    return resp.data;
  }
  get(id: string, signal?: AbortSignal): Promise<AdminModelView> {
    return this.client.request("GET", `/v1/admin/models/${encodeURIComponent(id)}`, undefined, signal);
  }
  /** Add a model at runtime. The body is a full model schema, validated like `models/*.yaml`. */
  create(schema: ModelSchema, signal?: AbortSignal): Promise<AdminModelView> {
    return this.client.request("POST", "/v1/admin/models", schema, signal);
  }
  /** Replace a runtime model's schema. Catalog models are read-only. */
  replace(id: string, schema: ModelSchema, signal?: AbortSignal): Promise<AdminModelView> {
    return this.client.request("PUT", `/v1/admin/models/${encodeURIComponent(id)}`, schema, signal);
  }
  /** Enable or disable any model platform-wide. */
  setDisabled(id: string, disabled: boolean, signal?: AbortSignal): Promise<AdminModelView> {
    return this.client.request("PATCH", `/v1/admin/models/${encodeURIComponent(id)}`, { disabled }, signal);
  }
  delete(id: string, signal?: AbortSignal): Promise<DeletedResponse> {
    return this.client.request("DELETE", `/v1/admin/models/${encodeURIComponent(id)}`, undefined, signal);
  }
}

class AdminProvidersNamespace {
  constructor(private readonly client: LiteGenClient) {}
  /** Every provider adapter and its effective config. Secrets come back masked. */
  async list(signal?: AbortSignal): Promise<ProviderConfigView[]> {
    const resp = await this.client.request<ProviderConfigListResponse>("GET", "/v1/admin/providers", undefined, signal);
    return resp.data;
  }
  get(name: string, signal?: AbortSignal): Promise<ProviderConfigView> {
    return this.client.request("GET", `/v1/admin/providers/${encodeURIComponent(name)}`, undefined, signal);
  }
  /** Set keys, api_base, model mapping, headers, options or the enabled flag at runtime. */
  configure(name: string, config: PutProviderConfigRequest, signal?: AbortSignal): Promise<ProviderConfigView> {
    return this.client.request("PUT", `/v1/admin/providers/${encodeURIComponent(name)}`, config, signal);
  }
  /** Drop the runtime config; the provider falls back to its env/YAML config. */
  reset(name: string, signal?: AbortSignal): Promise<ProviderConfigView> {
    return this.client.request("DELETE", `/v1/admin/providers/${encodeURIComponent(name)}`, undefined, signal);
  }
}

class AdminRoutesNamespace {
  constructor(private readonly client: LiteGenClient) {}
  get(signal?: AbortSignal): Promise<RoutesDocument> {
    return this.client.request("GET", "/v1/admin/routes", undefined, signal);
  }
  /** Replace every model route. The first matching route wins. */
  replace(routes: RouteSpec[], signal?: AbortSignal): Promise<RoutesDocument> {
    return this.client.request("PUT", "/v1/admin/routes", { routes }, signal);
  }
  /** Go back to the routes in litegen.yaml. */
  reset(signal?: AbortSignal): Promise<RoutesDocument> {
    return this.client.request("DELETE", "/v1/admin/routes", undefined, signal);
  }
}

class AdminSettingsNamespace {
  constructor(private readonly client: LiteGenClient) {}
  get(signal?: AbortSignal): Promise<GatewaySettingsView> {
    return this.client.request("GET", "/v1/admin/settings", undefined, signal);
  }
  /** Replace the gateway settings; an omitted field is cleared. */
  update(settings: GatewaySettings, signal?: AbortSignal): Promise<GatewaySettingsView> {
    return this.client.request("PUT", "/v1/admin/settings", settings, signal);
  }
}

class AdminNamespace {
  readonly models: AdminModelsNamespace;
  readonly providers: AdminProvidersNamespace;
  readonly routes: AdminRoutesNamespace;
  readonly settings: AdminSettingsNamespace;
  constructor(client: LiteGenClient) {
    this.models = new AdminModelsNamespace(client);
    this.providers = new AdminProvidersNamespace(client);
    this.routes = new AdminRoutesNamespace(client);
    this.settings = new AdminSettingsNamespace(client);
  }
}
