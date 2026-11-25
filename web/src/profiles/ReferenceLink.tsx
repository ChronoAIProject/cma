import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { useProfileViewsClient } from '../api/context';
import { connectionUrl } from '../connections/client';
import type { AppRoute, ProfileKind } from '../routing/routes';
import { routePath } from '../routing/routes';
import type { ServiceCatalogEntry } from './client';

type ReferenceKind = ProfileKind | 'service' | 'skill';
type Resolved = {
  readonly name: string;
  readonly detailUrl?: string;
  readonly publicProfile?: boolean;
};
type Resolve = (kind: ReferenceKind, id: string) => Promise<Resolved | undefined>;
const References = createContext<
  | {
      resolve: Resolve;
      peek: (kind: ReferenceKind, id: string) => Resolved | undefined;
      navigate: (route: AppRoute) => void;
    }
  | undefined
>(undefined);

export function ReferenceProvider({
  children,
  onNavigate,
}: {
  readonly children: ReactNode;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const profiles = useProfileViewsClient();
  const retained = useRef<
    | {
        client: typeof profiles;
        cache: Map<string, Promise<Resolved | undefined>>;
        names: Map<string, Resolved>;
        services?: Promise<readonly ServiceCatalogEntry[]>;
      }
    | undefined
  >(undefined);
  const resolve = useCallback<Resolve>(
    (kind, id) => {
      if (retained.current?.client !== profiles || retained.current === undefined)
        retained.current = { client: profiles, cache: new Map(), names: new Map() };
      const directory = retained.current;
      const cache = directory.cache;
      const key = `${kind}:${id}`;
      const previous = cache.get(key);
      if (previous !== undefined) return previous;
      const read = async () => {
        if (profiles === undefined) return undefined;
        if (kind === 'service') {
          directory.services ??= profiles.authoring?.listServices();
          return (await directory.services)?.find(
            (entry) => entry.id === id || entry.catalogId === id,
          );
        }
        if (kind === 'skill') {
          const page = await profiles.authoring?.searchSkills(id);
          return page?.items.find((entry) => entry.id === id);
        }
        try {
          const profile = await profiles[kind].readProfile(id);
          return profile.id === id ? { name: profile.name } : undefined;
        } catch {
          const profile = await profiles.marketplace.readMarketplace(kind, id);
          return profile.summary.id === id
            ? { name: profile.summary.name, publicProfile: true }
            : undefined;
        }
      };
      const pending = read()
        .then((value) => {
          if (value !== undefined) directory.names.set(key, value);
          return value;
        })
        .catch(() => {
          cache.delete(key);
          if (kind === 'service') directory.services = undefined;
          return undefined;
        });
      cache.set(key, pending);
      return pending;
    },
    [profiles],
  );
  const peek = useCallback(
    (kind: ReferenceKind, id: string) => {
      const directory = retained.current;
      return directory?.client === profiles ? directory?.names.get(`${kind}:${id}`) : undefined;
    },
    [profiles],
  );
  return <References value={{ resolve, peek, navigate: onNavigate }}>{children}</References>;
}

export function ReferenceLink({
  kind,
  id,
  name,
  version,
  catalogId,
  detailUrl,
}: {
  readonly kind: ReferenceKind;
  readonly id: string;
  readonly name?: string;
  readonly version?: string;
  readonly catalogId?: string;
  readonly detailUrl?: string;
}) {
  const context = useContext(References);
  const [resolved, setResolved] = useState<{ key: string; resolver: Resolve; value?: Resolved }>();
  const resolver = context?.resolve;
  const key = `${kind}:${id}`;
  useEffect(() => {
    if (
      resolver === undefined ||
      (name !== undefined && (kind === 'skill' || (kind === 'service' && catalogId !== undefined)))
    )
      return;
    let current = true;
    void resolver(kind, id).then((value) => {
      if (current) setResolved({ key, resolver, value });
    });
    return () => {
      current = false;
    };
  }, [resolver, kind, id, key, name, catalogId]);
  const settled = resolved?.key === key && resolved.resolver === resolver;
  const value = settled ? resolved.value : context?.peek(kind, id);
  const label = name ?? value?.name;
  const fallback =
    kind === 'service'
      ? 'Service unavailable'
      : kind === 'skill'
        ? 'Skill unavailable'
        : 'Profile unavailable';
  const loading = name === undefined && context !== undefined && !settled && value === undefined;
  const route: AppRoute | undefined =
    kind === 'service' || kind === 'skill'
      ? undefined
      : {
          kind: value?.publicProfile ? 'marketplace_profile' : 'profile',
          profileKind: kind,
          profileId: id,
        };
  const catalogUrl = detailUrl ?? value?.detailUrl;
  const href =
    route !== undefined
      ? routePath(route)
      : kind === 'service'
        ? catalogUrl && connectionUrl(catalogUrl)
        : `https://ornn.chrono-ai.fun/skills/${encodeURIComponent(id)}${version === undefined ? '' : `?version=${encodeURIComponent(version)}`}`;
  if (!href) return <span>{label ?? (loading ? 'Loading name' : fallback)}</span>;
  return (
    <a
      href={href}
      target={route === undefined ? '_blank' : undefined}
      rel={route === undefined ? 'noopener noreferrer' : undefined}
      onClick={(event) => {
        if (
          route === undefined ||
          context === undefined ||
          event.button !== 0 ||
          event.metaKey ||
          event.ctrlKey ||
          event.altKey ||
          event.shiftKey
        )
          return;
        event.preventDefault();
        context.navigate(route);
      }}
    >
      {label ?? (loading ? 'Loading name' : fallback)}
    </a>
  );
}
