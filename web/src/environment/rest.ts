import { createClient, toEnvironmentProfile } from '@chronoai/cma-sdk';
import type { EnvironmentProfile, EnvironmentProfileStore, ProfileDraft } from './types';

/**
 * Dashboard adapter over the published profile client.
 *
 * The package owns every route, wire name, validation rule, deadline, and
 * problem document. This file only presents those operations through the
 * dashboard's existing store port.
 */

export function toProfile(raw: unknown): EnvironmentProfile {
  return toEnvironmentProfile(raw);
}

export function createRestProfileStore(): EnvironmentProfileStore {
  const cma = createClient();
  return {
    list: () => cma.listEnvironmentProfiles(),
    create: (draft: ProfileDraft) => cma.createEnvironmentProfile(draft),
    update: (id: string, draft: ProfileDraft) => cma.updateEnvironmentProfile(id, draft),
    remove: (id: string) => cma.deleteEnvironmentProfile(id),
    prefer: (id: string | null) => cma.preferEnvironmentProfile(id),
    catalogue: () => cma.readSkillCatalogue(),
    services: () => cma.readServiceCatalogue(),
  };
}
