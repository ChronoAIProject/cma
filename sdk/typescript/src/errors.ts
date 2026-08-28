/** Base class for every failure reported by the CMA SDK. */
export abstract class CmaError extends Error {
  protected constructor(name: string, message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = name;
  }
}

/** The service answered with a non-success HTTP response. */
export class ApiError extends CmaError {
  constructor(
    readonly status: number,
    message: string,
    /** Stable problem code, where the service supplied one. */
    readonly code?: string,
    /** Correlation id returned with the failed response, when usable. */
    readonly requestId?: string,
    /** Validated positive Retry-After delay supplied by the service. */
    readonly retryAfterMs?: number,
  ) {
    super('ApiError', message);
  }

  get isUnauthenticated(): boolean {
    return this.status === 401;
  }

  /** Whether a later request could plausibly receive a different answer. */
  get isRetryable(): boolean {
    return (
      (this.status === 409 && this.code === 'session_rotating') ||
      this.status === 429 ||
      this.status >= 500
    );
  }
}

/** The session permanently lacks the named capability. */
export class CapabilityRefused extends ApiError {
  constructor(
    status: number,
    message: string,
    readonly capability: string,
    requestId?: string,
    retryAfterMs?: number,
  ) {
    super(status, message, 'capability_unsupported', requestId, retryAfterMs);
    this.name = 'CapabilityRefused';
  }
}

/** The owner already holds the deployment's maximum live sessions. */
export class SessionLimitReached extends ApiError {
  constructor(
    status: number,
    message: string,
    readonly limit?: number,
    readonly live?: number,
    requestId?: string,
    retryAfterMs?: number,
  ) {
    super(status, message, 'session_limit_reached', requestId, retryAfterMs);
    this.name = 'SessionLimitReached';
  }
}

/** The deployment has less declared capacity than a creation requests. */
export class DeploymentAtCapacity extends ApiError {
  constructor(
    status: number,
    message: string,
    readonly requestedCpuMillis?: number,
    readonly requestedMemoryMib?: number,
    readonly freeCpuMillis?: number,
    readonly freeMemoryMib?: number,
    requestId?: string,
    retryAfterMs?: number,
  ) {
    super(status, message, 'deployment_at_capacity', requestId, retryAfterMs);
    this.name = 'DeploymentAtCapacity';
  }
}

/** The service or stream crossed the boundary with an invalid wire shape. */
export class ProtocolError extends CmaError {
  constructor(message: string, options?: ErrorOptions) {
    super('ProtocolError', message, options);
  }
}

/** The caller supplied a value that cannot be represented on the wire. */
export class ValidationError extends CmaError {
  constructor(message: string, options?: ErrorOptions) {
    super('ValidationError', message, options);
  }
}

/** A credential callback failed before a request could be sent. */
export class CredentialError extends CmaError {
  constructor(message: string, options?: ErrorOptions) {
    super('CredentialError', message, options);
  }
}

/** A configured request-id provider failed or returned an unusable value. */
export class RequestIdError extends CmaError {
  constructor(message: string, options?: ErrorOptions) {
    super('RequestIdError', message, options);
  }
}

/** A response observer failed after the service answered. */
export class ResponseObserverError extends CmaError {
  constructor(options?: ErrorOptions) {
    super('ResponseObserverError', 'the response observer failed', options);
  }
}

/** The caller deliberately cancelled an operation. */
export class RequestAborted extends CmaError {
  constructor(message = 'the request was aborted by its caller', options?: ErrorOptions) {
    super('RequestAborted', message, options);
  }
}

/** No valid HTTP answer could be obtained. */
export class TransportError extends CmaError {
  constructor(message: string, options?: ErrorOptions) {
    super('TransportError', message, options);
  }

  readonly isRetryable = true;
}

/** An operation exceeded its configured deadline. */
export class TimeoutError extends TransportError {
  constructor(
    readonly timeoutMs: number,
    options?: ErrorOptions,
  ) {
    super(`the request exceeded its ${String(timeoutMs)}ms timeout`, options);
    this.name = 'TimeoutError';
  }
}
