import type { components } from "./generated/schema";

type ErrorDetail = components["schemas"]["ErrorDetail"];

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
  readonly providerError?: unknown;

  constructor(status: number, detail: ErrorDetail) {
    super(detail.message);
    this.name = "LiteGenAPIError";
    this.status = status;
    this.type = detail.type;
    // The server emits `code` as a number in some paths and a string in others,
    // while the declared type is `string`. Coerce so `error.code === "429"`
    // comparisons behave as the type promises, regardless of the wire form.
    this.code = detail.code != null ? String(detail.code) : undefined;
    this.providerError = detail.provider_error ?? undefined;
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
  constructor(id: string, lastStatus?: string) {
    super(
      `Polling for video '${id}' timed out${lastStatus ? ` (last status: ${lastStatus})` : ""}`,
    );
    this.name = "LiteGenPollingTimeoutError";
    this.lastStatus = lastStatus;
  }
}
