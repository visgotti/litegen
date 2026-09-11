import type { components } from "./generated/schema";
import { LiteGenAPIError, LiteGenTimeoutError } from "./errors";
import {
  waitForCompletion,
  pollVideo,
  poll3d,
  waitForJob,
  MODEL3D_KIND,
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
  /** Which media this provider serves — any of "image", "video". */
  modalities: string[];
  /** Coarse auth scheme: "api_key" | "aws_sigv4" | "tencent_tc3" | "kling_jwt". */
  auth_scheme: string;
  /** Which credentials array to submit: "api_keys" (bearer) or "credential_sets" (signing). */
  pool_field: string;
  /** Fields to collect per pool entry, besides the universal weight/label. */
  fields: CredentialFieldSpec[];
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
        body: body !== undefined ? JSON.stringify(body) : undefined,
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
    const res = await this.fetchOk(method, path, body, signal);
    // 204 No Content and similar empty responses — return undefined cast to T.
    if (res.status === 204 || res.headers.get("content-length") === "0") {
      return undefined as unknown as T;
    }
    return (await res.json()) as T;
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
 * There's no SSE on the wire — it submits then polls under the hood. Use
 * `await job.submitted` if you only want the initial job id without waiting.
 * Submission is lazy + memoized: the POST fires on first use.
 */
export class VideoJob implements PromiseLike<VideoResponse>, AsyncIterable<VideoResponse> {
  private submittedPromise?: Promise<VideoResponse>;

  constructor(
    private readonly submitFn: () => Promise<VideoResponse>,
    private readonly pollFn: (id: string) => AsyncGenerator<VideoResponse, VideoResponse, void>,
  ) {}

  /** The initial submit response (id, status) — does not wait for completion. */
  get submitted(): Promise<VideoResponse> {
    if (!this.submittedPromise) this.submittedPromise = this.submitFn();
    return this.submittedPromise;
  }

  then<TResult1 = VideoResponse, TResult2 = never>(
    onfulfilled?: ((value: VideoResponse) => TResult1 | PromiseLike<TResult1>) | null,
    onrejected?: ((reason: unknown) => TResult2 | PromiseLike<TResult2>) | null,
  ): Promise<TResult1 | TResult2> {
    return this.runToCompletion().then(onfulfilled, onrejected);
  }

  async *[Symbol.asyncIterator](): AsyncGenerator<VideoResponse, void, void> {
    const job = await this.submitted;
    yield* this.pollFn(job.id);
  }

  private async runToCompletion(): Promise<VideoResponse> {
    const job = await this.submitted;
    let last: VideoResponse = job;
    for await (const update of this.pollFn(job.id)) last = update;
    return last;
  }
}

class VideosNamespace {
  constructor(private readonly client: LiteGenClient) {}
  /**
   * Submit a video job. The returned {@link VideoJob} is awaitable (resolves to
   * the finished job) and async-iterable (streams progress).
   */
  generate(req: VideoRequest, opts?: WaitForCompletionOptions): VideoJob {
    return new VideoJob(
      () =>
        this.client.request<VideoResponse>(
          "POST",
          "/v1/videos/generations",
          req,
          opts?.signal,
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
 * const mesh = await client.models3d.generate({ ... });
 * const url = mesh.assets?.find(a => a.kind === "mesh")?.url;
 *
 * for await (const update of client.models3d.generate({ ... })) {
 *   console.log(`${update.status} — ${update.progress}%`);
 * }
 * ```
 *
 * A completed job always carries exactly one `kind: "mesh"` asset with an
 * absolute URL; its absence means the generation failed — there is no
 * separate failure payload to check.
 */
export class Model3dJob implements PromiseLike<Model3dResponse>, AsyncIterable<Model3dResponse> {
  private submittedPromise?: Promise<Model3dResponse>;

  constructor(
    private readonly submitFn: () => Promise<Model3dResponse>,
    private readonly pollFn: (id: string) => AsyncGenerator<Model3dResponse, Model3dResponse, void>,
  ) {}

  /** The initial submit response (id, status) — does not wait for completion. */
  get submitted(): Promise<Model3dResponse> {
    if (!this.submittedPromise) this.submittedPromise = this.submitFn();
    return this.submittedPromise;
  }

  then<TResult1 = Model3dResponse, TResult2 = never>(
    onfulfilled?: ((value: Model3dResponse) => TResult1 | PromiseLike<TResult1>) | null,
    onrejected?: ((reason: unknown) => TResult2 | PromiseLike<TResult2>) | null,
  ): Promise<TResult1 | TResult2> {
    return this.runToCompletion().then(onfulfilled, onrejected);
  }

  async *[Symbol.asyncIterator](): AsyncGenerator<Model3dResponse, void, void> {
    const job = await this.submitted;
    yield* this.pollFn(job.id);
  }

  private async runToCompletion(): Promise<Model3dResponse> {
    const job = await this.submitted;
    let last: Model3dResponse = job;
    for await (const update of this.pollFn(job.id)) last = update;
    return last;
  }
}

class Models3dNamespace {
  constructor(private readonly client: LiteGenClient) {}
  /**
   * Submit a 3D job. The returned {@link Model3dJob} is awaitable (resolves to
   * the finished job) and async-iterable (streams progress).
   */
  generate(req: Model3dRequest, opts?: WaitForCompletionOptions): Model3dJob {
    return new Model3dJob(
      () =>
        this.client.request<Model3dResponse>(
          "POST",
          "/v1/models3d/generations",
          req,
          opts?.signal,
        ),
      (id) => poll3d(id, (jid) => this.getStatus(jid, opts?.signal), opts),
    );
  }
  estimateCost(req: Model3dRequest, signal?: AbortSignal): Promise<CostEstimate> {
    return this.client.request("POST", "/v1/models3d/cost", req, signal);
  }
  getStatus(id: string, signal?: AbortSignal): Promise<Model3dResponse> {
    return this.client.request("GET", `/v1/models3d/${encodeURIComponent(id)}`, undefined, signal);
  }
  waitForCompletion(id: string, opts?: WaitForCompletionOptions): Promise<Model3dResponse> {
    return waitForJob(id, (jid) => this.getStatus(jid, opts?.signal), opts, MODEL3D_KIND);
  }
  /**
   * Stream progress updates for a 3D job as an async iterable:
   * `for await (const update of client.models3d.poll(job.id)) { ... }`.
   * Yields each poll (incl. the terminal one); `update.progress` is the unified
   * 0–100 value.
   */
  poll(
    id: string,
    opts?: WaitForCompletionOptions,
  ): AsyncGenerator<Model3dResponse, Model3dResponse, void> {
    return poll3d(id, (jid) => this.getStatus(jid, opts?.signal), opts);
  }
}

class ModelsNamespace {
  constructor(private readonly client: LiteGenClient) {}
  async list(signal?: AbortSignal): Promise<ModelInfo[]> {
    const resp = await this.client.request<ModelListResponse>(
      "GET",
      "/v1/models",
      undefined,
      signal,
    );
    return resp.data;
  }
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
