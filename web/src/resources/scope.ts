import type { AppRoute } from '../routing/routes';

// Picker values keep scope kinds separate from arbitrary organization IDs.
export function scopeValue(organization?: string, shared?: boolean): string {
  return shared === true
    ? 'shared'
    : organization === undefined
      ? ''
      : `organization:${organization}`;
}

export function scopeRoute(value: string): Extract<AppRoute, { kind: 'inventory' }> {
  if (value === 'shared') return { kind: 'inventory', shared: true };
  return {
    kind: 'inventory',
    organization: value.startsWith('organization:') ? value.slice(13) : undefined,
  };
}
