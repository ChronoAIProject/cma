import { useCallback, useEffect, useState } from 'react';

export type ProfileKind = 'agent' | 'workspace' | 'sandbox';

export type AppRoute =
  | { readonly kind: 'home' }
  | { readonly kind: 'agent_manager' }
  | { readonly kind: 'artifacts' }
  | { readonly kind: 'document'; readonly documentId: string }
  | { readonly kind: 'event_gateway' }
  | { readonly kind: 'image_manager' }
  | { readonly kind: 'docs'; readonly view?: 'event_gateway_protocol' }
  | { readonly kind: 'sandbox'; readonly sandboxId: string }
  | { readonly kind: 'workspace'; readonly workspaceId: string }
  | { readonly kind: 'agent'; readonly agentId: string }
  | { readonly kind: 'agent_share'; readonly shareId: string }
  | { readonly kind: 'inventory'; readonly organization?: string; readonly shared?: boolean }
  | { readonly kind: 'bots' }
  | { readonly kind: 'bot_new' }
  | { readonly kind: 'bot'; readonly botId: string }
  | { readonly kind: 'profiles' }
  | { readonly kind: 'event_worker_profiles' }
  | { readonly kind: 'event_worker_profile'; readonly profileId: string }
  | { readonly kind: 'profile'; readonly profileKind: ProfileKind; readonly profileId: string }
  | { readonly kind: 'marketplace' }
  | {
      readonly kind: 'marketplace_profile';
      readonly profileKind: ProfileKind;
      readonly profileId: string;
    }
  | { readonly kind: 'triggers' }
  | { readonly kind: 'trigger'; readonly triggerId: string }
  | { readonly kind: 'trigger_embed'; readonly triggerId: string }
  | { readonly kind: 'public_trigger'; readonly publicId: string; readonly placementId?: string }
  | { readonly kind: 'launch'; readonly launchId: string }
  | { readonly kind: 'operations' }
  | { readonly kind: 'settings'; readonly tab: 'general' | 'usage' }
  | { readonly kind: 'not_found'; readonly pathname: string };

const PROFILE_KINDS: readonly ProfileKind[] = ['agent', 'workspace', 'sandbox'];

function isProfileKind(value: string): value is ProfileKind {
  return (PROFILE_KINDS as readonly string[]).includes(value);
}

function decodeSegment(value: string): string | null {
  if (value === '') return null;
  try {
    const decoded = decodeURIComponent(value);
    return decoded !== '' && decoded !== '.' && decoded !== '..' && !decoded.includes('/')
      ? decoded
      : null;
  } catch {
    return null;
  }
}

export function parseRoute(pathname: string): AppRoute {
  if (pathname === '/' || pathname === '') return { kind: 'home' };
  if (!pathname.startsWith('/') || pathname.endsWith('/')) {
    return { kind: 'not_found', pathname };
  }

  const raw = pathname.slice(1).split('/');
  const segments = raw.map(decodeSegment);
  if (segments.some((pathSegment) => pathSegment === null)) {
    return { kind: 'not_found', pathname };
  }
  const path = segments as string[];

  if (path.length === 1) {
    switch (path[0]) {
      case 'event-gateway':
        return { kind: 'event_gateway' };
      case 'image-manager':
        return { kind: 'not_found', pathname };
      case 'artifacts':
        return { kind: 'artifacts' };
      case 'docs':
        return { kind: 'docs' };
      case 'agent-manager':
        return { kind: 'agent_manager' };
      case 'agents':
        return { kind: 'inventory' };
      case 'bots':
        return { kind: 'bots' };
      case 'profiles':
        return { kind: 'profiles' };
      case 'marketplace':
        return { kind: 'marketplace' };
      case 'operations':
        return { kind: 'operations' };
      case 'settings':
        return { kind: 'settings', tab: 'general' };
      case 'developer':
        // Retired: the SDK reader it named no longer exists. Old links land on
        // the documentation that replaced it, and the address is rewritten.
        return { kind: 'docs' };
      default:
        return { kind: 'not_found', pathname };
    }
  }

  if (path.length === 2) {
    const [resource = '', id = ''] = path;
    if (resource === 'settings' && (id === 'general' || id === 'usage'))
      return { kind: 'settings', tab: id };
    if (resource === 'docs' && id === 'event-gateway')
      return { kind: 'docs', view: 'event_gateway_protocol' };
    if (resource === 'shared' && id === 'agents') return { kind: 'inventory', shared: true };
    if (resource === 'artifacts' && /^doc_[a-f0-9]{32}$/.test(id))
      return { kind: 'document', documentId: id };
    if (resource === 'sandboxes') return { kind: 'sandbox', sandboxId: id };
    if (resource === 'workspaces') return { kind: 'workspace', workspaceId: id };
    if (resource === 'agents') return { kind: 'agent', agentId: id };
    if (resource === 'bots') return id === 'new' ? { kind: 'bot_new' } : { kind: 'bot', botId: id };
    if (resource === 'profiles' && id === 'event-workers') return { kind: 'event_worker_profiles' };
    if (resource === 'launches') return { kind: 'launch', launchId: id };
    if (resource === 't') return { kind: 'public_trigger', publicId: id };
    if (resource === 'triggers' && id === 'manage') return { kind: 'triggers' };
  }

  if (path.length === 3) {
    const [resource = '', qualifier = '', id = ''] = path;
    if (resource === 'agent-manager' && qualifier === 'share')
      return { kind: 'agent_share', shareId: id };
    if (resource === 'organizations' && id === 'agents')
      return { kind: 'inventory', organization: qualifier };
    if (resource === 'profiles' && qualifier === 'event-workers')
      return { kind: 'event_worker_profile', profileId: id };
    if (resource === 'profiles') {
      const profileKind = PROFILE_KINDS.find((kind) => `${kind}-profiles` === qualifier);
      if (profileKind !== undefined) return { kind: 'profile', profileKind, profileId: id };
    }
    if (resource === 'marketplace' && isProfileKind(qualifier)) {
      return { kind: 'marketplace_profile', profileKind: qualifier, profileId: id };
    }
    if (resource === 'triggers' && qualifier === 'manage') {
      return { kind: 'trigger', triggerId: id };
    }
    if (resource === 't') {
      return { kind: 'public_trigger', publicId: qualifier, placementId: id };
    }
  }

  if (path.length === 4 && path[0] === 'triggers' && path[1] === 'manage' && path[3] === 'embed') {
    return { kind: 'trigger_embed', triggerId: path[2] ?? '' };
  }

  return { kind: 'not_found', pathname };
}

function segment(value: string): string {
  return encodeURIComponent(value);
}

export function routePath(route: AppRoute): string {
  switch (route.kind) {
    case 'home':
      return '/';
    case 'agent_manager':
      return '/agent-manager';
    case 'event_gateway':
      return '/event-gateway';
    case 'image_manager':
      return '/image-manager';
    case 'artifacts':
      return '/artifacts';
    case 'document':
      return `/artifacts/${segment(route.documentId)}`;
    case 'docs':
      return route.view === 'event_gateway_protocol' ? '/docs/event-gateway' : '/docs';
    case 'sandbox':
      return `/sandboxes/${segment(route.sandboxId)}`;
    case 'workspace':
      return `/workspaces/${segment(route.workspaceId)}`;
    case 'agent':
      return `/agents/${segment(route.agentId)}`;
    case 'agent_share':
      return `/agent-manager/share/${segment(route.shareId)}`;
    case 'inventory':
      if (route.shared === true) return '/shared/agents';
      return route.organization === undefined
        ? '/agents'
        : `/organizations/${segment(route.organization)}/agents`;
    case 'bots':
      return '/bots';
    case 'bot_new':
      return '/bots/new';
    case 'bot':
      return `/bots/${segment(route.botId)}`;
    case 'profiles':
      return '/profiles';
    case 'event_worker_profiles':
      return '/profiles/event-workers';
    case 'event_worker_profile':
      return `/profiles/event-workers/${segment(route.profileId)}`;
    case 'profile':
      return `/profiles/${route.profileKind}-profiles/${segment(route.profileId)}`;
    case 'marketplace':
      return '/marketplace';
    case 'marketplace_profile':
      return `/marketplace/${route.profileKind}/${segment(route.profileId)}`;
    case 'triggers':
      return '/triggers/manage';
    case 'trigger':
      return `/triggers/manage/${segment(route.triggerId)}`;
    case 'trigger_embed':
      return `/triggers/manage/${segment(route.triggerId)}/embed`;
    case 'public_trigger': {
      const base = `/t/${segment(route.publicId)}`;
      return route.placementId === undefined ? base : `${base}/${segment(route.placementId)}`;
    }
    case 'launch':
      return `/launches/${segment(route.launchId)}`;
    case 'operations':
      return '/operations';
    case 'settings':
      return `/settings/${route.tab}`;
    case 'not_found':
      return route.pathname;
  }
}

export interface AppRouter {
  readonly route: AppRoute;
  readonly navigate: (route: AppRoute, options?: { readonly replace?: boolean }) => void;
}

/** Addresses that still parse, to their replacement, but are never shown. */
const RETIRED_PATHS: ReadonlySet<string> = new Set(['/developer']);

/**
 * Read the address bar, rewriting a retired path to the one that replaced it.
 *
 * Replace rather than push: the retired address was never a place somebody
 * chose to be, so Back should not return to it.
 */
function currentRoute(): AppRoute {
  const route = parseRoute(window.location.pathname);
  if (RETIRED_PATHS.has(window.location.pathname)) {
    window.history.replaceState(window.history.state, '', routePath(route));
  }
  return route;
}

export function useAppRouter(): AppRouter {
  const [route, setRoute] = useState<AppRoute>(currentRoute);

  useEffect(() => {
    const onPopState = () => {
      setRoute(currentRoute());
    };
    window.addEventListener('popstate', onPopState);
    return () => {
      window.removeEventListener('popstate', onPopState);
    };
  }, []);

  const navigate = useCallback<AppRouter['navigate']>((next, options) => {
    const path = routePath(next);
    if (options?.replace === true) {
      window.history.replaceState(null, '', path);
    } else {
      window.history.pushState(null, '', path);
    }
    setRoute(parseRoute(path));
  }, []);

  return { route, navigate };
}
