import { useMemo } from 'react';
import { useSessionReadSignal } from '../auth/context';
import type { ResourceViewsClient } from './client';
import { useAuthorizedRead } from './useAuthorizedRead';
import { scopeRoute, scopeValue } from './scope';
import { Picker } from '../ui/Picker';
import type { OrganizationChoice } from '@cma/client';

export interface NavigationScopeChoices {
  readonly options: readonly { readonly value: string; readonly label: string }[];
  readonly selected?: OrganizationChoice;
  readonly unavailable: boolean;
  readonly reload?: () => void;
  readonly loading?: boolean;
}

export function useNavigationScope(
  client: ResourceViewsClient | undefined,
  scope: string,
): NavigationScopeChoices {
  const sessionSignal = useSessionReadSignal();
  const read = useMemo(
    () =>
      client === undefined || sessionSignal?.aborted
        ? undefined
        : async (signal: AbortSignal) => {
            const requestSignal = sessionSignal ? AbortSignal.any([signal, sessionSignal]) : signal;
            requestSignal.throwIfAborted();
            return {
              value: (await client.resourceAccess.organizations({ signal: requestSignal })).items,
            };
          },
    [client, sessionSignal],
  );
  const current = useAuthorizedRead(read);
  const { organization } = scopeRoute(scope);
  const organizations = current.state.status === 'ready' ? current.state.value : [];
  const selected = organizations.find((choice) => choice.id === organization);
  const options = [
    { value: '', label: 'Personal' },
    { value: 'shared', label: 'Shared' },
    ...organizations.map((choice, index) => ({
      value: scopeValue(choice.id),
      label: `${choice.display_name?.trim() || `Organization ${index + 1}`} (Organization)`,
    })),
  ];
  if (organization !== undefined && selected === undefined)
    options.push({
      value: scopeValue(organization),
      label: 'Selected organization (Organization)',
    });
  return {
    options,
    selected,
    unavailable: current.state.status === 'error',
    reload: current.reload,
    loading: current.state.status === 'loading' || current.refreshing,
  };
}

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
      placement="navigation-filters__picker"
      value={value}
      options={choices.options}
      onChange={onChange}
    />
  );
}
