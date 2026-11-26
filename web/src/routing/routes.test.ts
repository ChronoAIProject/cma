import { act, renderHook } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { parseRoute, routePath, useAppRouter, type AppRoute } from './routes';

const ROUTES: readonly AppRoute[] = [
  { kind: 'home' },
  { kind: 'agent_manager' },
  { kind: 'event_gateway' },
  { kind: 'docs' },
  { kind: 'docs', view: 'event_gateway_protocol' },
  { kind: 'docs', view: 'start-a-task' },
  { kind: 'docs', view: 'environments-and-capacity' },
  { kind: 'docs', view: 'recover-and-continue' },
  { kind: 'docs', view: 'find-your-outputs' },
  { kind: 'bots' },
  { kind: 'bot_new' },
  { kind: 'bot', botId: 'bot_123' },
  { kind: 'sandbox', sandboxId: 'sbx_123' },
  { kind: 'workspace', workspaceId: 'wks_123' },
  { kind: 'agent', agentId: 'agt_123' },
  { kind: 'agent_share', shareId: 'shr_123' },
  { kind: 'inventory' },
  { kind: 'inventory', shared: true },
  { kind: 'inventory', organization: 'org_123' },
  { kind: 'profiles' },
  { kind: 'profile', profileKind: 'agent', profileId: 'agp_123' },
  { kind: 'profile', profileKind: 'workspace', profileId: 'wkp_123' },
  { kind: 'profile', profileKind: 'sandbox', profileId: 'sbp_123' },
  { kind: 'marketplace' },
  { kind: 'marketplace_profile', profileKind: 'agent', profileId: 'agp_123' },
  { kind: 'triggers' },
  { kind: 'trigger', triggerId: 'trg_123' },
  { kind: 'trigger_embed', triggerId: 'trg_123' },
  { kind: 'public_trigger', publicId: 'pub_123' },
  { kind: 'public_trigger', publicId: 'pub_123', placementId: 'plc_123' },
  { kind: 'launch', launchId: 'lch_123' },
  { kind: 'operations' },
  { kind: 'usage' },
];

describe('application routes', () => {
  it.each(ROUTES)('round trips $kind', (route) => {
    expect(parseRoute(routePath(route))).toEqual(route);
  });

  it('encodes opaque identifiers without changing their value', () => {
    const route = { kind: 'agent', agentId: 'agent name?#' } as const;
    expect(routePath(route)).toBe('/agents/agent%20name%3F%23');
    expect(parseRoute(routePath(route))).toEqual(route);
  });

  it.each([
    'agents/agt_1',
    '/agents/',
    '/agents/a/b',
    '/agents/%',
    '/agents/%2F',
    '/agents/.',
    '/profiles/agent',
    '/profiles/agent/agp_1',
    '/profiles/unknown/id',
    '/triggers/manage/id/unknown',
    '/operations/extra',
    '/docs/unknown-topic',
    '/docs/start-a-task/extra',
  ])('rejects malformed or unknown path %s', (pathname) => {
    expect(parseRoute(pathname)).toEqual({ kind: 'not_found', pathname });
  });

  it('preserves an unknown path for rendering and browser history', () => {
    expect(routePath({ kind: 'not_found', pathname: '/unknown' })).toBe('/unknown');
  });

  it.each(['agent', 'workspace', 'sandbox'] as const)(
    'uses the explicit %s editor resource path',
    (profileKind) => {
      const route = { kind: 'profile' as const, profileKind, profileId: 'profile-one' };
      expect(routePath(route)).toBe(`/profiles/${profileKind}-profiles/profile-one`);
      expect(parseRoute(routePath(route))).toEqual(route);
    },
  );

  it('tracks navigate, replace, and browser popstate from the current location', () => {
    window.history.replaceState(null, '', '/');
    const { result } = renderHook(() => useAppRouter());

    act(() => {
      result.current.navigate({ kind: 'agent', agentId: 'agt_1' });
    });
    expect(window.location.pathname).toBe('/agents/agt_1');
    expect(result.current.route).toEqual({ kind: 'agent', agentId: 'agt_1' });

    act(() => {
      window.history.replaceState(null, '', '/sandboxes/sbx_1');
      window.dispatchEvent(new PopStateEvent('popstate'));
    });
    expect(result.current.route).toEqual({ kind: 'sandbox', sandboxId: 'sbx_1' });

    act(() => {
      result.current.navigate({ kind: 'operations' }, { replace: true });
    });
    expect(window.location.pathname).toBe('/operations');
    expect(result.current.route).toEqual({ kind: 'operations' });
  });

  it('rewrites the retired Developer address to Docs without a history entry', () => {
    window.history.replaceState(null, '', '/agent-manager');
    window.history.pushState(null, '', '/developer');
    const entries = window.history.length;
    const { result } = renderHook(() => useAppRouter());
    expect(result.current.route).toEqual({ kind: 'docs' });
    expect(window.location.pathname).toBe('/docs');
    expect(window.history.length).toBe(entries);
  });

  it('leaves an unknown address as written', () => {
    window.history.replaceState(null, '', '/developers');
    const { result } = renderHook(() => useAppRouter());
    expect(result.current.route).toEqual({ kind: 'not_found', pathname: '/developers' });
    expect(window.location.pathname).toBe('/developers');
  });
});

it('refuses direct Image Manager URLs after retirement', () => {
  expect(parseRoute('/image-manager')).toEqual({ kind: 'not_found', pathname: '/image-manager' });
  expect(parseRoute('/image-manager/images')).toEqual({
    kind: 'not_found',
    pathname: '/image-manager/images',
  });
});

it.each(['/settings', '/settings/general', '/settings/usage'])(
  'replaces legacy %s without changing theme or discarding the query',
  (path) => {
    localStorage.setItem('cma:appearance', 'light');
    window.history.replaceState(null, '', path + '?range=7d&sandbox=sbx-2');
    const entries = window.history.length;
    const { result } = renderHook(() => useAppRouter());
    expect(result.current.route).toEqual({ kind: 'usage' });
    expect(result.current.backgroundRoute).toEqual({ kind: 'agent_manager' });
    expect(result.current.usageAppearanceNotice).toBe(path !== '/settings/usage');
    expect(window.location.pathname).toBe('/usage');
    expect(window.location.search).toBe('?range=7d&sandbox=sbx-2');
    expect(window.history.length).toBe(entries);
    expect(localStorage.getItem('cma:appearance')).toBe('light');
  },
);
it('keeps the background route when opening and replaying Usage history', () => {
  window.history.replaceState(null, '', '/agents/agt_1?view=chat');
  const { result } = renderHook(() => useAppRouter());
  const background = result.current.route;
  act(() => result.current.navigate({ kind: 'usage' }));
  const saved = window.history.state;
  expect(result.current.backgroundRoute).toBe(background);
  expect(saved.usageBackground).toBe('/agents/agt_1?view=chat');
  act(() => {
    window.history.replaceState(null, '', '/agents/agt_1?view=chat');
    window.dispatchEvent(new PopStateEvent('popstate'));
  });
  expect(result.current.route).toEqual(background);
  act(() => {
    window.history.replaceState(saved, '', '/usage');
    window.dispatchEvent(new PopStateEvent('popstate'));
  });
  expect(result.current.backgroundRoute).toEqual(background);
});
it('closes a direct Usage link to the safe app shell by replacement', () => {
  window.history.replaceState(null, '', '/usage');
  const entries = window.history.length;
  const { result } = renderHook(() => useAppRouter());
  act(() => result.current.closeUsage?.());
  expect(result.current.route).toEqual({ kind: 'agent_manager' });
  expect(window.history.length).toBe(entries);
});
