import './Settings.css';
import { useMemo } from 'react';
import { useProfileViewsClient } from '../api/context';
import { ServicePicker } from '../profiles/pickers/ServicePicker';
import { REQUIRED_AGENT_SCOPES } from './SettingsFields';

export function ScopePicker({
  value,
  onChange,
  disabled,
  fixed = REQUIRED_AGENT_SCOPES,
  requested = [],
  replace = false,
}: {
  readonly value: readonly string[];
  readonly onChange: (services: readonly string[]) => void;
  readonly disabled: boolean;
  readonly fixed?: readonly string[];
  readonly requested?: readonly string[];
  readonly replace?: boolean;
}) {
  const authoring = useProfileViewsClient()?.authoring;
  const signature = JSON.stringify([
    ...new Set([...REQUIRED_AGENT_SCOPES, ...fixed, ...requested]),
  ]);
  const required = useMemo(() => JSON.parse(signature) as string[], [signature]);
  const catalog = useMemo(
    () =>
      authoring === undefined
        ? undefined
        : {
            listServices: async (...args: Parameters<typeof authoring.listServices>) =>
              (await authoring.listServices(...args)).filter(
                (entry) => !required.includes(entry.id),
              ),
          },
    [authoring, required],
  );
  return (
    <fieldset className="agent-settings-scopes">
      <legend>Agent key scope</legend>
      <p className="profile-field__help">
        These services are shared by all Agents in this Sandbox.
      </p>
      <ServicePicker
        fixed={required.map((id) => ({
          id,
          reason: REQUIRED_AGENT_SCOPES.some((baseline) => baseline === id)
            ? 'Required'
            : fixed.includes(id)
              ? 'Already granted'
              : 'Requested',
        }))}
        help={
          replace
            ? 'Select the services this Sandbox may use. Unchecking a service revokes access when you save.'
            : 'Choose additional services this task needs. Existing Sandbox permissions remain available.'
        }
        catalog={catalog}
        value={value.filter((scope) => !required.includes(scope))}
        onChange={(services) => onChange([...required, ...services])}
        disabled={disabled}
      />
    </fieldset>
  );
}
