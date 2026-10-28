import { expect } from 'vitest';
import { ApiError, ValidationError, errorFromResponse, type Cma } from '../../src/index.js';
import type { ContractCase } from './context.js';

export const browserOperations = new Set([
  'GET /auth/login',
  'GET /auth/logout',
  'GET /auth/callback',
  'GET /api/v1/triggers/public/{public_id}/open/{placement_id}',
  'GET /api/v1/agents/{id}/desktop/ws',
]);

/** Navigation URLs are SDK output; browser redirects and callback handling remain composition-only. */
export async function verifyNavigation(
  client: Cma,
  fixture: ContractCase,
  response: Response,
): Promise<void> {
  const url = new URL(fixture.request.path, client.config.baseUrl);
  if (fixture.operation === 'GET /auth/login') {
    const build = () =>
      client.auth.loginUrl({
        returnTo: url.searchParams.get('return_to') ?? undefined,
        screen: (url.searchParams.get('screen') ?? undefined) as 'register' | undefined,
      });
    if (
      ['auth-login-open-redirect-refused', 'auth-login-retired-return-refused'].includes(fixture.id)
    )
      expect(build).toThrow(ValidationError);
    else {
      const actual = new URL(build());
      expect(actual.pathname).toBe(url.pathname);
      expect([...actual.searchParams].sort()).toEqual([...url.searchParams].sort());
    }
  } else if (fixture.operation === 'GET /auth/logout') {
    expect(new URL(client.auth.logoutUrl()).pathname).toBe('/auth/logout');
  } else if (fixture.operation === 'GET /api/v1/triggers/public/{public_id}/open/{placement_id}') {
    const parts = url.pathname.split('/');
    expect(parts[5]).toBeTruthy();
    expect(parts[6]).toBe('open');
    expect(parts[7]).toMatch(/^plc_/);
    expect(fixture.response.headers.location).toBe(`/t/${parts[5]}/${parts[7]}`);
    expect(fixture.response.headers['cache-control']).toBe('private, no-store');
    expect(fixture.response.headers['set-cookie']).toContain('Max-Age=0');
  } else {
    expect(fixture.operation).toBe('GET /auth/callback');
    // The callback is consumed by CMA's server, never by an SDK-issued token exchange.
    expect(url.pathname).toBe('/auth/callback');
  }
  if (fixture.response.status >= 400) {
    const problem = await errorFromResponse(response);
    expect(problem).toBeInstanceOf(ApiError);
    expect(problem).toMatchObject({
      status: fixture.response.status,
      code: (fixture.response.body as { code?: string }).code,
    });
  } else {
    expect(fixture.response.status).toBe(303);
    expect(fixture.response.headers.location).toMatch(/^\/(?!\/)/);
    expect(fixture.response.bodyKind).toBe('empty');
  }
}
