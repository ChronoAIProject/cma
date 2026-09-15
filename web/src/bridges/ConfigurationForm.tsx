import type { BridgeConfiguration, BridgeConnectorChoices } from '@chronoai/cma-sdk';
import type { BridgeViewsClient } from './client';
import { ConnectionFields } from './ConnectionFields';
import { ContextFields } from './ContextFields';
import { PolicyFields } from './PolicyFields';

export function ConfigurationForm({
  client,
  value,
  choices,
  disabled,
  existing = false,
  organization,
  onChange,
}: {
  readonly client: BridgeViewsClient;
  readonly value: BridgeConfiguration;
  readonly choices?: BridgeConnectorChoices;
  readonly disabled: boolean;
  readonly existing?: boolean;
  readonly organization?: string;
  readonly onChange: (value: BridgeConfiguration) => void;
}) {
  return (
    <div className="bridges__form">
      <ConnectionFields
        value={value}
        choices={choices}
        disabled={disabled}
        existing={existing}
        onChange={onChange}
      />
      <ContextFields
        client={client}
        value={value}
        disabled={disabled}
        organization={organization}
        onChange={onChange}
      />
      <PolicyFields value={value} disabled={disabled} onChange={onChange} />
    </div>
  );
}
