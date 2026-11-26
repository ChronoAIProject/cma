import {
  ApiError,
  errorFromBody,
  readBytes,
  responseRequestId,
  retryAfterMilliseconds,
} from '@cma/client';
import { observeRetryAfter, type ReadRetryMetadata } from '../readRetry';

export interface BusinessReadProblem extends ReadRetryMetadata {
  readonly status: number;
  readonly code?: string;
}

export class BusinessReadError extends ApiError {
  constructor(readonly problem: BusinessReadProblem) {
    super(
      problem.status,
      `Agent lifecycle status failed (${String(problem.status)}).`,
      problem.code,
      problem.requestId,
      problem.retryAfterMs,
      Object.freeze(problem.retryable === undefined ? {} : { retryable: problem.retryable }),
    );
  }
}

/** Decode only the bounded failure contract; upstream presentation stays private. */
export async function readBusinessProblem(
  response: Response,
  signal?: AbortSignal,
): Promise<BusinessReadProblem> {
  const retryAfterMs = retryAfterMilliseconds(response);
  const retryAt = observeRetryAfter(retryAfterMs, signal);
  const common = {
    status: response.status,
    retryAfterMs,
    retryAt,
    requestId: responseRequestId(response),
  };
  const controller = new AbortController();
  const abort = () => controller.abort(signal?.reason);
  const timer = setTimeout(() => controller.abort(), 1000);
  signal?.addEventListener('abort', abort, { once: true });
  let rejectAborted: (() => void) | undefined;
  try {
    signal?.throwIfAborted();
    const cancelled = new Promise<never>((_resolve, reject) => {
      rejectAborted = () => reject(controller.signal.reason);
      controller.signal.addEventListener('abort', rejectAborted, { once: true });
    });
    const bytes = await Promise.race([
      readBytes(response, controller.signal, 64 * 1024),
      cancelled,
    ]);
    signal?.throwIfAborted();
    const text = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
    if (text.trim() === '') return Object.freeze({ ...common, shape: 'legacy-empty' });
    const raw: unknown = JSON.parse(text);
    if (typeof raw !== 'object' || raw === null || Array.isArray(raw))
      return Object.freeze({ ...common, shape: 'invalid' });
    const value = raw as Record<string, unknown>;
    const retryable = typeof value.retryable === 'boolean' ? value.retryable : undefined;
    const code =
      typeof value.code === 'string' && /^[a-z][a-z0-9_]{0,127}$/.test(value.code)
        ? value.code
        : undefined;
    const invalid =
      ('status' in value && value.status !== response.status) ||
      ('retryable' in value && typeof value.retryable !== 'boolean') ||
      ('code' in value && code === undefined) ||
      ['type', 'title', 'detail', 'instance'].some(
        (key) => key in value && typeof value[key] !== 'string',
      ) ||
      (code === undefined &&
        !['title', 'detail'].some(
          (key) => typeof value[key] === 'string' && value[key].trim() !== '',
        ));
    if (invalid) return Object.freeze({ ...common, code, retryable, shape: 'invalid' });
    const decoded = errorFromBody(
      response,
      {
        status: response.status,
        code,
        detail: 'Read unavailable.',
        ...(retryable === undefined ? {} : { retryable }),
      },
      common.requestId,
    );
    return Object.freeze({
      ...common,
      code: decoded instanceof ApiError ? decoded.code : code,
      retryable,
      shape: decoded instanceof ApiError ? 'problem' : 'invalid',
    });
  } catch {
    if (signal?.aborted) throw signal.reason;
    return Object.freeze({ ...common, shape: 'invalid' });
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener('abort', abort);
    if (rejectAborted) controller.signal.removeEventListener('abort', rejectAborted);
    if (response.body && !response.body.locked) void response.body.cancel().catch(() => undefined);
  }
}
