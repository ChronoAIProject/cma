import type { ReactNode } from 'react';
import { CmaClientProvider } from '../api/context';
import { unavailableClient } from '../api/client';
import { ReferenceProvider } from '../profiles/ReferenceLink';
import type { ProfileViewsClient } from '../profiles/ProfileViews';
import {
  AGENT_PROFILE,
  SANDBOX_PROFILE,
  WORKSPACE_PROFILE,
  documentClient,
  marketplaceClient,
  OWN_PROFILES,
  PROFILE_BOUNDS,
} from './profiles';

export function referenceClient(
  catalogOrigin = 'https://catalog.example.test',
): ProfileViewsClient {
  return {
    agent: {
      ...documentClient(AGENT_PROFILE),
      readProfile: async (id) => ({ ...AGENT_PROFILE, id }),
    },
    workspace: {
      ...documentClient(WORKSPACE_PROFILE),
      readProfile: async (id) => ({ ...WORKSPACE_PROFILE, id }),
    },
    sandbox: {
      ...documentClient(SANDBOX_PROFILE),
      readProfile: async (id) => ({ ...SANDBOX_PROFILE, id }),
    },
    profiles: OWN_PROFILES,
    marketplace: marketplaceClient(),
    authoring: {
      readCapacityBounds: async () => PROFILE_BOUNDS,
      listServices: async () => [
        {
          id: 'github',
          name: 'GitHub',
          catalogId: 'catalog-github',
          detailUrl: `${catalogOrigin}/services/catalog-github`,
        },
        {
          id: 'storage',
          name: 'Chrono Storage',
          catalogId: 'catalog-storage',
          detailUrl: `${catalogOrigin}/services/catalog-storage`,
        },
      ],
      searchSkills: async (id) => ({ items: [{ id, name: 'TypeScript', versions: ['1.2'] }] }),
      resolveRepository: async (value) => value,
    },
  };
}

export function NamedReferences({
  children,
  catalogOrigin,
}: {
  readonly children: ReactNode;
  readonly catalogOrigin?: string;
}) {
  return (
    <CmaClientProvider client={unavailableClient} profiles={referenceClient(catalogOrigin)}>
      <ReferenceProvider onNavigate={() => {}}>{children}</ReferenceProvider>
    </CmaClientProvider>
  );
}
