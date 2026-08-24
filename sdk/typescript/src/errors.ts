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
  ) {
    super('ApiError', message);
  }

  get isUnauthenticated(): boolean {
    return this.status === 401;
  }

  /** Whether a later request could plausibly receive a different answer. */
  get isRetryable(): boolean {
    return this.status === 429 || this.status >= 500;
  }
}

/** The session permanently lacks the named capability. */
export class CapabilityRefused extends ApiError {
  constructor(
    status: number,
    message: string,
    readonly capability: string,
  ) {
    super(status, message, 'capability_unsupported');
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
  ) {
    super(status, message, 'session_limit_reached');
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
  ) {
    super(status, message, 'deployment_at_capacity');
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
