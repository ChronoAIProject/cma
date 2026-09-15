import type { EffectiveAccess, OrganizationChoice } from '@chronoai/cma-sdk';

/** Resource permissions, never the caller's organization membership title. */
export function accessRole(access: EffectiveAccess) {
  const can = (capability: string) => access.capabilities.includes(capability);
  if (!can('view_details')) return 'Access unavailable';
  if (access.owner?.kind === 'person' && access.owner.is_current_person && can('manage_sharing'))
    return 'Owner';
  if (can('manage_sharing')) return 'Resource admin';
  if (can('manage_sandbox')) return 'Sandbox manager';
  if (can('operate_agent')) return 'Agent operator';
  if (can('interact')) return 'Can interact';
  return 'View only';
}

export function ownerLabel(access: EffectiveAccess, organizations: readonly OrganizationChoice[]) {
  if (access.owner === undefined) return 'Owner unavailable';
  if (access.owner.kind === 'person')
    return access.owner.is_current_person ? 'You' : 'Personal owner';
  const id = access.owner.organization_id;
  return (
    organizations.find((choice) => choice.id === id)?.display_name?.trim() || 'Organization owner'
  );
}

export interface AccessSummary {
  readonly owner: string;
  readonly role: string;
}

export function summarizeAccess(
  access: EffectiveAccess,
  organizations: readonly OrganizationChoice[],
): AccessSummary {
  return { owner: ownerLabel(access, organizations), role: accessRole(access) };
}
