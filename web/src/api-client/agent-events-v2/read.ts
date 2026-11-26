import type { ResolvedConfig } from '../config.js';
import { ProtocolError } from '../errors.js';
import {
  raceWithAbortSignal,
  requestResponse,
  withDeadline,
  type OperationOptions,
} from '../http.js';
import { MAX_AGENT_BYTES } from './schema.js';

export function readAgentJson(
  config: ResolvedConfig,
  path: string,
  options: OperationOptions,
  headers?: HeadersInit,
  onHeaders?: (headers: Headers) => void,
): Promise<unknown> {
  return withDeadline(config, options, async (signal, requestId) => {
    const response = await requestResponse(config, path, {
      ...options,
      headers,
      signal,
      requestId,
      expectedStatus: 200,
    });
    if (
      response.headers.get('content-type')?.split(';')[0]?.trim().toLowerCase() !==
        'application/json' ||
      response.body === null
    )
      throw new ProtocolError('Agent read requires an application/json body');
    try {
      onHeaders?.(response.headers);
    } catch (error) {
      void response.body.cancel().catch(() => undefined);
      throw error;
    }
    const length = response.headers.get('content-length');
    if (length !== null && Number(length) > MAX_AGENT_BYTES) {
      void response.body.cancel().catch(() => undefined);
      throw new ProtocolError('Agent page exceeds its byte boundary');
    }
    const reader = response.body.getReader();
    const decoder = new TextDecoder('utf-8', { fatal: true });
    let bytes = 0;
    let body = '';
    try {
      while (true) {
        const { done, value } = await raceWithAbortSignal(signal, () => reader.read());
        if (value !== undefined) {
          bytes += value.byteLength;
          if (bytes > MAX_AGENT_BYTES)
            throw new ProtocolError('Agent page exceeds its byte boundary');
          body += decoder.decode(value, { stream: true });
        }
        if (done) break;
      }
      body += decoder.decode();
      return JSON.parse(body) as unknown;
    } catch (cause) {
      if (signal.aborted || cause instanceof ProtocolError) throw cause;
      throw new ProtocolError('Agent read returned invalid UTF-8 or JSON', { cause });
    } finally {
      void reader.cancel().catch(() => undefined);
      reader.releaseLock();
    }
  });
}
