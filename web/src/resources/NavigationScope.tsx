import { useMemo } from 'react';
import type { ResourceViewsClient } from './client';
import { useAuthorizedRead } from './useAuthorizedRead';
import { scopeRoute, scopeValue } from './scope';
import { Picker } from '../ui/Picker';

export function useNavigationScope(client: ResourceViewsClient | undefined, scope: string) {
  const read = useMemo(
    () =>
      client === undefined
        ? undefined
        : async (signal: AbortSignal) => ({
            value: (await client.resourceAccess.organizations({ signal })).items,
          }),
    [client],
  );
  const current = useAuthorizedRead(read);
  const { organization } = scopeRoute(scope);
  const organizations = current.state.status === 'ready' ? current.state.value : [];
  const selected = organizations.find((choice) => choice.id === organization);
  const options = [
    { value: '', label: 'Personal' },
    { value: 'shared', label: 'Shared with me' },
    ...organizations.map((choice, index) => ({
      value: scopeValue(choice.id),
      label: choice.display_name?.trim() || `Organization ${index + 1}`,
    })),
  ];
  if (organization !== undefined && selected === undefined)
    options.push({ value: scopeValue(organization), label: 'Selected organization' });
  return { options, selected, unavailable: current.state.status === 'error' };
}

export type NavigationScopeChoices = ReturnType<typeof useNavigationScope>;

export function NavigationScopePicker({
  choices,
  value,
  onChange,
}: {
  readonly choices: NavigationScopeChoices;
  readonly value: string;
  readonly onChange: (value: string) => void;
}) {
  return (
    <Picker
      label="Navigation scope"
      placement="sidebar__scope-picker"
      value={value}
      options={choices.options}
      onChange={onChange}
    />
  );
}
