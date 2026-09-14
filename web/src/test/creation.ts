import type { CreationClient, CreationProfile } from '../create/client';
import { fromMarketplace } from '../api/creation';
import { MARKETPLACE_SANDBOX } from './profiles';

export const CREATION_SANDBOX = fromMarketplace(MARKETPLACE_SANDBOX);
export const CREATION_AGENT: CreationProfile = {
  ...MARKETPLACE_SANDBOX.resolvedRevisions[1]!,
  requiresConfirmation: false,
  disclosures: [],
};
export const CREATION_WORKSPACE: CreationProfile = {
  ...MARKETPLACE_SANDBOX.resolvedRevisions[0]!,
  requiresConfirmation: true,
  disclosures: [{ name: 'Research workspace', commands: ['mkdir -p reports'], environment: [] }],
};

export function creationClient(overrides: Partial<CreationClient> = {}): CreationClient {
  const profiles = {
    agent: CREATION_AGENT,
    workspace: CREATION_WORKSPACE,
    sandbox: CREATION_SANDBOX,
  };
  return {
    listProfiles: async (kind, source) => ({
      items: [
        { id: profiles[kind].revision.profileId, revision: 1, source, name: profiles[kind].name },
      ],
    }),
    readProfile: async (kind) => profiles[kind],
    ...overrides,
  };
}
