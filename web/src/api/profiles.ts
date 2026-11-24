import { createCreationClient } from './creation';
import type { Cma } from '@cma/client';
import type { ProfileViewsClient } from '../profiles/ProfileViews';
import { agentContent, sandboxContent, workspaceContent } from './rest/profile-content';
import { profileEditor, profileSummary } from './rest/profile-documents';
import { marketplaceClient } from './rest/marketplace';

export function createProfileViewsClient(sdk: Cma): ProfileViewsClient {
  const resources = {
    agent: sdk.agentProfiles,
    workspace: sdk.workspaceProfiles,
    sandbox: sdk.sandboxProfiles,
  };
  return {
    eventWorkers: sdk.eventWorkerProfiles,
    creation: createCreationClient(sdk),
    profiles: {
      listProfiles: async (kind, options) => {
        const page = await resources[kind].list(options);
        return { items: page.items.map(profileSummary), next: page.next };
      },
    },
    agent: profileEditor(sdk.agentProfiles, agentContent),
    workspace: profileEditor(sdk.workspaceProfiles, workspaceContent),
    sandbox: profileEditor(sdk.sandboxProfiles, sandboxContent),
    marketplace: marketplaceClient(sdk),
    authoring: {
      readCapacityBounds: async (options) => {
        const bounds = await sdk.profileAuthoring.bounds(options);
        return {
          minCpuMillis: bounds.min_cpu_millis,
          maxCpuMillis: bounds.max_cpu_millis,
          minMemoryMib: bounds.min_memory_mib,
          maxMemoryMib: bounds.max_memory_mib,
        };
      },
      listServices: async (options) =>
        (await sdk.profileAuthoring.services(options)).map((entry) => ({
          id: entry.id,
          catalogId: entry.catalog_id,
          detailUrl: entry.detail_url,
          name: entry.name,
          description: entry.description,
        })),
      searchSkills: (query, options) =>
        sdk.profileAuthoring.searchSkills({ q: query, cursor: options?.cursor }, options),
      resolveRepository: ({ url, reference }, options) =>
        sdk.profileAuthoring.resolveRepository({ url, reference }, options),
    },
  };
}
