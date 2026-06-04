import type { components } from "./generated/schema";
import { LiteGenAPIError, LiteGenTimeoutError } from "./errors";
import { waitForCompletion, pollVideo, type WaitForCompletionOptions } from "./polling";

type Schemas = components["schemas"];
type ImageRequest = Schemas["ImageGenerationRequest"];
type ImageResponse = Schemas["ImageGenerationResponse"];
type VideoRequest = Schemas["VideoGenerationRequest"];
type VideoResponse = Schemas["VideoGenerationResponse"];
type CostEstimate = Schemas["CostEstimate"];
type ModelInfo = Schemas["ModelInfo"];
type ModelSchema = Schemas["ModelSchema"];
type ModelListResponse = Schemas["ModelListResponse"];
type HealthResponse = Schemas["HealthResponse"];
type LivenessResponse = Schemas["LivenessResponse"];
type ProxyStats = Schemas["ProxyStats"];
type RequestLog = Schemas["RequestLog"];
type ApiKeyInfo = Schemas["ApiKeyInfo"];
type ApiKeyListResponse = Schemas["ApiKeyListResponse"];
type ApiKeyCreatedResponse = Schemas["ApiKeyCreatedResponse"];
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

export interface Generation {
  id: string;
  key_id: string;
  model: string;
  provider: string;
  status: "pending" | "processing" | "completed" | "failed" | "cancelled";
  cost_usd: number;
  created_at: string;
  completed_at?: string;
  result_url?: string;
  request_body?: Record<string, unknown>;
  error?: string;
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

export interface GetLogsOptions {
  page?: number;
  per_page?: number;
  model?: string;
  provider?: string;
  status?: string;
  from?: string;
  to?: string;
}

export interface ListWebhookDeliveriesOptions {
  page?: number;
  per_page?: number;
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
  readonly models: ModelsNamespace;
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
    this.models = new ModelsNamespace(this);
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

  /** @internal */
  async request<T>(
    method: string,
    path: string,
    body?: unknown,
    signal?: AbortSignal,
  ): Promise<T> {
    const url = `${this.baseUrl}${path}`;
    const headers: Record<string, string> = {
      "Content-Type": "application/json",
      ...this.defaultHeaders,
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
    // 204 No Content and similar empty responses — return undefined cast to T.
    if (res.status === 204 || res.headers.get("content-length") === "0") {
      return undefined as unknown as T;
    }
    return (await res.json()) as T;
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

class LogsNamespace {
  constructor(private readonly client: LiteGenClient) {}
  list(
    opts: GetLogsOptions = {},
    signal?: AbortSignal,
  ): Promise<PaginatedResponse<RequestLog>> {
    const { page = 1, per_page = 50, model, provider, status, from, to } = opts;
    const params = new URLSearchParams({ page: String(page), per_page: String(per_page) });
    if (model) params.set("model", model);
    if (provider) params.set("provider", provider);
    if (status) params.set("status", status);
    if (from) params.set("from", from);
    if (to) params.set("to", to);
    return this.client.request(
      "GET",
      `/v1/logs?${params.toString()}`,
      undefined,
      signal,
    );
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

class KeysNamespace {
  constructor(private readonly client: LiteGenClient) {}
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
  patch(id: string, body: PatchKeyBody, signal?: AbortSignal): Promise<ApiKeyInfo> {
    return this.client.request("PATCH", `/v1/keys/${encodeURIComponent(id)}`, body, signal);
  }
  revoke(id: string, signal?: AbortSignal): Promise<RevokeKeyResponse> {
    return this.client.request("DELETE", `/v1/keys/${encodeURIComponent(id)}`, undefined, signal);
  }
  /** Alias: plan calls `client.keys.delete` */
  delete(id: string, signal?: AbortSignal): Promise<RevokeKeyResponse> {
    return this.revoke(id, signal);
  }
  rotate(id: string, signal?: AbortSignal): Promise<ApiKeyCreatedResponse> {
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
    const { page = 1, per_page = 50, actor_key_id, action, from, to } = opts;
    const params = new URLSearchParams({ page: String(page), per_page: String(per_page) });
    if (actor_key_id) params.set("actor_key_id", actor_key_id);
    if (action) params.set("action", action);
    if (from) params.set("from", from);
    if (to) params.set("to", to);
    return this.client.request(
      "GET",
      `/v1/audit?${params.toString()}`,
      undefined,
      signal,
    );
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

class OrgsNamespace {
  readonly members: OrgMembersNamespace;
  readonly apps: OrgAppsNamespace;

  constructor(private readonly client: LiteGenClient) {
    this.members = new OrgMembersNamespace(client);
    this.apps = new OrgAppsNamespace(client);
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

class AppProviderCredentialsNamespace {
  constructor(private readonly client: LiteGenClient) {}

  list(appId: string, signal?: AbortSignal): Promise<ProviderCredentialInfo[]> {
    return this.client.request(
      "GET",
      `/v1/apps/${encodeURIComponent(appId)}/provider-credentials`,
      undefined,
      signal,
    );
  }
  create(
    appId: string,
    req: CreateProviderCredentialRequest,
    signal?: AbortSignal,
  ): Promise<ProviderCredentialInfo> {
    return this.client.request(
      "POST",
      `/v1/apps/${encodeURIComponent(appId)}/provider-credentials`,
      req,
      signal,
    );
  }
  delete(appId: string, provider: string, signal?: AbortSignal): Promise<void> {
    return this.client.request(
      "DELETE",
      `/v1/apps/${encodeURIComponent(appId)}/provider-credentials/${encodeURIComponent(provider)}`,
      undefined,
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

class AppsNamespace {
  readonly providerCredentials: AppProviderCredentialsNamespace;
  readonly storage: AppStorageNamespace;

  constructor(private readonly client: LiteGenClient) {
    this.providerCredentials = new AppProviderCredentialsNamespace(client);
    this.storage = new AppStorageNamespace(client);
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
