import { parseRoute, routePath, type AppRoute } from '../routing/routes';

const RETURN_TARGET_MAX_LENGTH = 256;
const RETURN_TARGET_SEGMENT = /^[A-Za-z0-9._~-]+$/;

/** Only UI-defined query state may accompany a canonical, unencoded path. */
export function canonicalReturnRoute(value: string): AppRoute | undefined {
  if (
    Array.from(value).some(
      (character) =>
        character.charCodeAt(0) <= 32 ||
        character.charCodeAt(0) >= 127 ||
        character === '#' ||
        character === '\\',
    )
  )
    return;
  const split = value.indexOf('?');
  const path = split < 0 ? value : value.slice(0, split);
  if (path.length > RETURN_TARGET_MAX_LENGTH || !path.startsWith('/')) return;
  if (
    path !== '/' &&
    path
      .slice(1)
      .split('/')
      .some(
        (segment) => segment === '.' || segment === '..' || !RETURN_TARGET_SEGMENT.test(segment),
      )
  )
    return;
  if (split >= 0) {
    const keys = new Set<string>();
    for (const [key] of new URLSearchParams(value.slice(split + 1))) {
      if (
        keys.has(key) ||
        !(
          (path === '/bots/new' && key === 'setup') ||
          (path.startsWith('/sandboxes/') && key === 'tab')
        )
      )
        return;
      keys.add(key);
    }
  }
  const route = parseRoute(path);
  return route.kind !== 'not_found' && routePath(route) === path ? route : undefined;
}

type ReturnTarget =
  { readonly kind: 'restore' | 'fallback'; readonly target: string } | { readonly kind: 'reject' };

export function classifyReturnTarget(value: string): ReturnTarget {
  switch (canonicalReturnRoute(value)?.kind) {
    case 'home':
    case 'agent_manager':
    case 'event_gateway':
    case 'agent':
    case 'sandbox':
    case 'bots':
    case 'bot_new':
    case 'bot':
    case 'profiles':
    case 'marketplace':
    case 'marketplace_profile':
    case 'triggers':
    case 'trigger':
    case 'trigger_embed':
    case 'public_trigger':
    case 'docs':
    case 'usage':
      return { kind: 'restore', target: value };
    case 'agent_share':
      return value.includes('?') ? { kind: 'reject' } : { kind: 'restore', target: value };
    case 'artifacts':
    case 'workspace':
    case 'inventory':
    case 'event_worker_profiles':
    case 'event_worker_profile':
    case 'profile':
    case 'operations':
      return { kind: 'fallback', target: '/agent-manager' };
    default:
      return { kind: 'reject' };
  }
}
