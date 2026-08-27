import { act, renderHook } from '@testing-library/react';
import { parseRoute, routePath, useAppRouter } from './routes';

describe('canonical routes', () => {
  it.each([
    ['/', { kind: 'home' }],
    ['/triggers/public%20one', { kind: 'trigger', publicId: 'public one' }],
    ['/launches/launch-1', { kind: 'launch', launchId: 'launch-1' }],
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

  it('round-trips encoded opaque identifiers', () => {
    const route = { kind: 'trigger', publicId: 'opaque value' } as const;
    expect(parseRoute(routePath(route))).toEqual(route);
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
});
