import { act, renderHook } from '@testing-library/react';
import { parseRoute, routePath, useAppRouter } from './routes';

describe('canonical routes', () => {
  it.each([
    ['/', { kind: 'home' }],
    ['/triggers/public%20one', { kind: 'trigger', publicId: 'public one' }],
    ['/launches/launch-1', { kind: 'launch', launchId: 'launch-1' }],
    ['/launches/launch-1/recover', { kind: 'trigger_recovery', launchId: 'launch-1' }],
    ['/launches/launch-1/contribute', { kind: 'contribute', launchId: 'launch-1' }],
    ['/sessions/session-1', { kind: 'session', sessionId: 'session-1' }],
  ])('parses %s', (pathname, expected) => {
    expect(parseRoute(pathname)).toEqual(expected);
  });

  it.each(['/triggers', '/triggers/a/extra', '/triggers/%2F', '/sessions/../launches/x'])(
    'refuses non-canonical path %s',
    (pathname) => {
      expect(parseRoute(pathname)).toEqual({ kind: 'not_found', pathname });
    },
  );

  it.each(['expired', 'cancelled', 'failed'] as const)(
    'retains the closed %s authentication-return error as display-only route state',
    (authError) => {
      expect(parseRoute('/triggers/public-one', `?auth_error=${authError}`)).toEqual({
        kind: 'trigger',
        publicId: 'public-one',
        authError,
      });
    },
  );

  it.each([
    '?auth_error=',
    '?auth_error=provider%20said%20something',
    '?auth_error=EXPIRED',
    '?auth_error=expired&auth_error=failed',
  ])('drops unrecognised or ambiguous authentication-return data from %s', (search) => {
    expect(parseRoute('/triggers/public-one', search)).toEqual({
      kind: 'trigger',
      publicId: 'public-one',
    });
  });

  it('round-trips encoded opaque identifiers', () => {
    const routes = [
      { kind: 'trigger', publicId: 'opaque value' },
      { kind: 'trigger_recovery', launchId: 'older launch' },
    ] as const;
    for (const route of routes) {
      expect(parseRoute(routePath(route))).toEqual(route);
    }
  });

  it('never includes an authentication-return error in a canonical destination', () => {
    expect(routePath({ kind: 'trigger', publicId: 'public-one', authError: 'failed' })).toBe(
      '/triggers/public-one',
    );
  });

  it('tracks push and browser history without performing an action', () => {
    window.history.replaceState(null, '', '/');
    const { result } = renderHook(() => useAppRouter());

    act(() => {
      result.current.navigate({ kind: 'launch', launchId: 'launch-1' });
    });
    expect(window.location.pathname).toBe('/launches/launch-1');
    expect(result.current.route).toEqual({ kind: 'launch', launchId: 'launch-1' });

    act(() => {
      window.history.replaceState(null, '', '/sessions/session-2');
      window.dispatchEvent(new PopStateEvent('popstate'));
    });
    expect(result.current.route).toEqual({ kind: 'session', sessionId: 'session-2' });
  });

  it('reads a typed authentication-return error and clears it on canonical navigation', () => {
    window.history.replaceState(null, '', '/triggers/public-one?auth_error=expired');
    const { result } = renderHook(() => useAppRouter());

    expect(result.current.route).toEqual({
      kind: 'trigger',
      publicId: 'public-one',
      authError: 'expired',
    });
    act(() => {
      result.current.navigate({ kind: 'trigger', publicId: 'public-one' }, { replace: true });
    });
    expect(window.location.search).toBe('');
    expect(result.current.route).toEqual({ kind: 'trigger', publicId: 'public-one' });
  });
});
