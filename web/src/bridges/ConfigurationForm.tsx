import type { BridgeConfiguration, BridgeConnectorChoices } from '@chronoai/cma-sdk';
import type { BridgeViewsClient } from './client';
import { BridgeCapabilityDetails } from './BridgeCapabilities';
import { ConnectionFields } from './ConnectionFields';
import { ContextFields } from './ContextFields';
import { PolicyFields } from './PolicyFields';
import type { PlacementEditor } from './usePlacementEditor';

export function ConfigurationForm({
  client,
  value,
  choices,
  editor,
  disabled,
  existing = false,
  organization,
  onChange,
}: {
  readonly client: BridgeViewsClient;
  readonly value: BridgeConfiguration;
  readonly choices?: BridgeConnectorChoices;
  readonly editor: PlacementEditor;
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
      <BridgeCapabilityDetails current={editor.capabilities} platform={value.binding.platform} />
      <ContextFields
        editor={editor}
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
