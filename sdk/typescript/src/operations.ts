import type { ResolvedConfig } from './config.js';
import { ProtocolError, ValidationError } from './errors.js';
import { request, requestVoid, type OperationOptions } from './http.js';

/** Text or raw terminal input. The session's granted plane decides its meaning. */
export type SessionInput =
  { readonly text: string; readonly newline?: boolean } | { readonly bytes: Uint8Array };

/** A pending approval's three wire decisions. */
export type ApprovalDecision = 'allow' | 'deny' | 'abort';

export interface TerminalGeometry {
  readonly cols: number;
  readonly rows: number;
}

export interface ScreenSnapshot extends TerminalGeometry {
  readonly lines: readonly string[];
}

/** Cumulative token and context usage reported by the runtime. */
export interface SessionUsage {
  readonly inputTokens: number;
  readonly outputTokens: number;
  readonly contextUsed: number;
  readonly contextWindow?: number;
}

/** Deliver input on whichever plane the session holds. */
export async function sendInput(
  config: ResolvedConfig,
  sessionId: string,
  input: SessionInput,
  options: OperationOptions = {},
): Promise<void> {
  await requestVoid(config, sessionPath(sessionId, 'input'), {
    ...options,
    method: 'POST',
    expectedStatus: 202,
    body: inputBody(input),
  });
}

/** Answer a pending approval without ending the session. */
export async function resolveApproval(
  config: ResolvedConfig,
  sessionId: string,
  requestId: string,
  decision: ApprovalDecision,
  options: OperationOptions = {},
): Promise<void> {
  if (!['allow', 'deny', 'abort'].includes(decision)) {
    throw new ValidationError("approval decision must be 'allow', 'deny', or 'abort'");
  }
  await requestVoid(
    config,
    `/api/v1/sessions/${encodeURIComponent(sessionId)}/approvals/${encodeURIComponent(requestId)}`,
    { ...options, method: 'POST', expectedStatus: 202, body: { decision } },
  );
}

/** Stop the in-flight turn without ending the session. */
export async function interrupt(
  config: ResolvedConfig,
  sessionId: string,
  options: OperationOptions = {},
): Promise<void> {
  await requestVoid(config, sessionPath(sessionId, 'interrupt'), {
    ...options,
    method: 'POST',
    expectedStatus: 202,
  });
}

/** Redirect a conversation turn already in progress. */
export async function steer(
  config: ResolvedConfig,
  sessionId: string,
  text: string,
  options: OperationOptions = {},
): Promise<void> {
  if (typeof text !== 'string') {
    throw new ValidationError('steering text must be a string');
  }
  await requestVoid(config, sessionPath(sessionId, 'steer'), {
    ...options,
    method: 'POST',
    expectedStatus: 202,
    body: { text },
  });
}

/** Inform a terminal session that its dimensions changed. */
export async function resizeTerminal(
  config: ResolvedConfig,
  sessionId: string,
  geometry: TerminalGeometry,
  options: OperationOptions = {},
): Promise<void> {
  const body = geometryOf(geometry, 'resize request');
  await requestVoid(config, sessionPath(sessionId, 'resize'), {
    ...options,
    method: 'POST',
    expectedStatus: 204,
    body,
  });
}

/** Fetch the current rendered terminal screen. */
export async function screenSnapshot(
  config: ResolvedConfig,
  sessionId: string,
  options: OperationOptions = {},
): Promise<ScreenSnapshot> {
  const raw = await request(config, sessionPath(sessionId, 'screen'), {
    ...options,
    expectedStatus: 200,
  });
  const body = objectOf(raw, 'screen response');
  const geometry = geometryOf(body, 'screen response', ProtocolError);
  if (!Array.isArray(body.lines) || !body.lines.every((line) => typeof line === 'string')) {
    throw new ProtocolError('screen response lines were not a list of strings');
  }
  if (body.lines.length !== geometry.rows) {
    throw new ProtocolError(
      `screen response declared ${String(geometry.rows)} rows but carried ${String(body.lines.length)} lines`,
    );
  }
  return { ...geometry, lines: body.lines };
}

/** Read cumulative token and context usage. */
export async function getUsage(
  config: ResolvedConfig,
  sessionId: string,
  options: OperationOptions = {},
): Promise<SessionUsage> {
  const raw = await request(config, sessionPath(sessionId, 'usage'), {
    ...options,
    expectedStatus: 200,
  });
  const body = objectOf(raw, 'usage response');
  const contextWindow = optionalInteger(body.context_window, 'usage context_window');
  return {
    inputTokens: integerOf(body.input_tokens, 'usage input_tokens'),
    outputTokens: integerOf(body.output_tokens, 'usage output_tokens'),
    contextUsed: integerOf(body.context_used, 'usage context_used'),
    ...(contextWindow === undefined ? {} : { contextWindow }),
  };
}

function inputBody(input: SessionInput): Record<string, unknown> {
  if (typeof input !== 'object' || input === null) {
    throw new ValidationError('session input must be text or bytes');
  }
  if ('bytes' in input) {
    if (!(input.bytes instanceof Uint8Array)) {
      throw new ValidationError('session byte input must be a Uint8Array');
    }
    return { bytes_b64: encodeBase64(input.bytes) };
  }
  if (!('text' in input) || typeof input.text !== 'string') {
    throw new ValidationError('session text input must carry a string');
  }
  if (input.newline !== undefined && typeof input.newline !== 'boolean') {
    throw new ValidationError('session input newline must be boolean');
  }
  return { text: input.text, newline: input.newline ?? true };
}

const BASE64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';

/** Cross-runtime base64 without relying on browser `btoa` or Node `Buffer`. */
function encodeBase64(bytes: Uint8Array): string {
  let encoded = '';
  for (let index = 0; index < bytes.length; index += 3) {
    const a = bytes[index] ?? 0;
    const b = bytes[index + 1] ?? 0;
    const c = bytes[index + 2] ?? 0;
    const word = (a << 16) | (b << 8) | c;
    encoded += BASE64.charAt((word >> 18) & 63);
    encoded += BASE64.charAt((word >> 12) & 63);
    encoded += index + 1 < bytes.length ? BASE64.charAt((word >> 6) & 63) : '=';
    encoded += index + 2 < bytes.length ? BASE64.charAt(word & 63) : '=';
  }
  return encoded;
}

function sessionPath(id: string, operation: string): string {
  return `/api/v1/sessions/${encodeURIComponent(id)}/${operation}`;
}

function objectOf(raw: unknown, name: string): Record<string, unknown> {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    throw new ProtocolError(`${name} was not an object`);
  }
  return raw as Record<string, unknown>;
}

function geometryOf(
  raw: { readonly cols?: unknown; readonly rows?: unknown },
  name: string,
  ErrorType: typeof ValidationError | typeof ProtocolError = ValidationError,
): TerminalGeometry {
  if (!dimension(raw.cols) || !dimension(raw.rows)) {
    throw new ErrorType(`${name} requires columns and rows between 1 and 65535`);
  }
  return { cols: raw.cols, rows: raw.rows };
}

function dimension(value: unknown): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value > 0 && value <= 65_535;
}

function integerOf(raw: unknown, name: string): number {
  if (typeof raw !== 'number' || !Number.isSafeInteger(raw) || raw < 0) {
    throw new ProtocolError(`${name} was not a non-negative safe integer`);
  }
  return raw;
}

function optionalInteger(raw: unknown, name: string): number | undefined {
  return raw === undefined || raw === null ? undefined : integerOf(raw, name);
}
