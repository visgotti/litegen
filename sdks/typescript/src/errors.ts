import type { components } from "./generated/schema";

type ErrorDetail = components["schemas"]["ErrorDetail"];
type VendorError = components["schemas"]["VendorError"];

export class LiteGenError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "LiteGenError";
  }
}

export class LiteGenAPIError extends LiteGenError {
  readonly status: number;
  readonly type: string;
  readonly code?: string;
  /**
   * The request field a validation error is about — `"output_formats"`,
   * `"reference_images"`, `"prompt"`, … — when the server names one. Branch on
   * this (with `code`) rather than parsing `message`.
   */
  readonly param?: string;
  /** The model the failed request named, on validation errors. */
  readonly model?: string;
  /**
   * The vendor's own answer when it refused or failed the request (`type`
   * `vendor_rejected` or `vendor_error`): its HTTP status, code and message.
   */
  readonly providerError?: VendorError;
  /** Whether the same request may work later (a vendor's 5xx, 429 or timeout). */
  readonly retryable?: boolean;
  /**
   * The wait the server's `Retry-After` header asked for (a 429 or 503), in
   * milliseconds; undefined when it sent none.
   */
  readonly retryAfterMs?: number;
  /**
   * The error response's headers — the rate limit a 429 reports, for one.
   * {@link readResponseMeta} reads litegen's out of them.
   */
  readonly headers?: Headers;

  constructor(status: number, detail: ErrorDetail, retryAfterMs?: number, headers?: Headers) {
    super(detail.message);
    this.name = "LiteGenAPIError";
    this.status = status;
    this.type = detail.type;
    // `code` is always a string on the wire (L15); coerce anyway so a stale
    // server's number still compares as the declared type promises.
    this.code = detail.code != null ? String(detail.code) : undefined;
    // The server writes `"param": null` when a validation error names no
    // field; normalise both to undefined so `err.param ?? …` reads naturally.
    this.param = typeof detail.param === "string" ? detail.param : undefined;
    this.model = typeof detail.model === "string" ? detail.model : undefined;
    this.providerError = detail.provider_error ?? undefined;
    this.retryable = typeof detail.retryable === "boolean" ? detail.retryable : undefined;
    this.retryAfterMs = retryAfterMs;
    this.headers = headers;
  }
}

export class LiteGenTimeoutError extends LiteGenError {
  constructor(message = "LiteGen request timed out") {
    super(message);
    this.name = "LiteGenTimeoutError";
  }
}

export class LiteGenPollingTimeoutError extends LiteGenError {
  readonly lastStatus?: string;
  /**
   * What timed out, as it reads in the message: "video", "3D generation", or
   * the neutral "job" when the caller polls a family this SDK has no name for.
   * `kind` is a trailing optional argument so the two-argument form this class
   * shipped with keeps working — it just reports the neutral word.
   */
  readonly kind: string;
  constructor(id: string, lastStatus?: string, kind = "job") {
    super(
      `Polling for ${kind} '${id}' timed out${lastStatus ? ` (last status: ${lastStatus})` : ""}`,
    );
    this.name = "LiteGenPollingTimeoutError";
    this.lastStatus = lastStatus;
    this.kind = kind;
  }
}
