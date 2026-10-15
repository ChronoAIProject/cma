import { act, renderHook } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { parseRoute, routePath, useAppRouter, type AppRoute } from './routes';

const ROUTES: readonly AppRoute[] = [
  { kind: 'home' },
  { kind: 'agent_manager' },
  { kind: 'event_gateway' },
  { kind: 'image_manager' },
  { kind: 'bots' },
  { kind: 'bot_new' },
  { kind: 'bot', botId: 'bot_123' },
  { kind: 'sandbox', sandboxId: 'sbx_123' },
  { kind: 'workspace', workspaceId: 'wks_123' },
  { kind: 'agent', agentId: 'agt_123' },
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
  { kind: 'developer' },
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
      result.current.navigate({ kind: 'developer' }, { replace: true });
    });
    expect(window.location.pathname).toBe('/developer');
    expect(result.current.route).toEqual({ kind: 'developer' });
  });
});
