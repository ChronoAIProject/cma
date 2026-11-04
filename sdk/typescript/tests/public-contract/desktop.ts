import { createHash } from 'node:crypto';
import { expect } from 'vitest';
import { ApiError, errorFromResponse } from '../../src/index.js';
import type { ContractCase } from './context.js';

/** The host/browser owns the WebSocket transport. Shared fixtures are replayed
 * against a real backend upgrade and binary relay by the backend contract suite. */
export async function verifyDesktopUpgrade(fixture: ContractCase): Promise<void> {
  expect(fixture.request.method).toBe('GET');
  expect(fixture.request.path).toMatch(/^\/api\/v1\/agents\/[^/]+\/desktop\/ws$/);
  const { status, bodyKind, body, headers } = fixture.response;
  if (status === 101) {
    expect(fixture.request.headers.upgrade).toBe('websocket');
    expect(fixture.request.headers['sec-websocket-version']).toBe('13');
    expect(headers.upgrade).toBe('websocket');
    expect(headers.connection).toBe('upgrade');
    expect(headers['sec-websocket-accept']).toBe(
      createHash('sha1')
        .update(
          fixture.request.headers['sec-websocket-key']! + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11',
        )
        .digest('base64'),
    );
    expect(bodyKind).toBe('empty');
    expect(body).toBeNull();
  } else if (bodyKind === 'json') {
    const error = await errorFromResponse(new Response(JSON.stringify(body), { status, headers }));
    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({ status, code: (body as { code: string }).code });
  } else if (status === 403) {
    expect(bodyKind).toBe('empty');
    expect(body).toBeNull();
  } else {
    expect(status).toBe(400);
    expect(bodyKind).toBe('text');
    expect(body).toBe("Connection header did not include 'upgrade'");
  }
}
