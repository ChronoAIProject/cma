import { useMemo, useState } from 'react';
import type { BridgePageProps } from './BridgePage';
import { useAuthorizedRead } from '../resources/useAuthorizedRead';
import { ResourceLink } from '../resources/ResourceLink';
import { Button } from '../ui/Button';
import { ConfigurationForm } from './ConfigurationForm';
import { usePlacementEditor } from './usePlacementEditor';
import { verifyPlacementForSave } from './placementSave';
import { initialConfiguration, configurationComplete } from './configuration';
import { useBridgeCommand, bridgeFailure } from './commands';

export function BridgeCreate({
  client,
  viewerId,
  organization,
  onNavigate,
}: BridgePageProps & { readonly organization?: string }) {
  const [config, setConfig] = useState(initialConfiguration);
  const [reviewed, setReviewed] = useState(false);
  const editor = usePlacementEditor(client, config, organization);
  const read = useMemo(
    () => async (signal: AbortSignal) => {
      try {
        return { value: await client.bridges.choices({ organization, signal }) };
      } catch (error) {
        throw new Error(bridgeFailure(error));
      }
    },
    [client, organization],
  );
  const connectors = useAuthorizedRead(read);
  const choices = connectors.state.status === 'ready' ? connectors.state.value : undefined;
  const command = useBridgeCommand(choices !== undefined);
  const disabled = choices === undefined || command.locked;
  return (
    <section aria-labelledby="bridge-create-title">
      <header className="bridges__header">
        <ResourceLink route={{ kind: 'bridges', organization }} onNavigate={onNavigate}>
          Back to bridges
        </ResourceLink>
        <h1 id="bridge-create-title">New bridge</h1>
        <p>
          {organization === undefined ? 'Personal bridge' : 'Organization bridge'} · Starts disabled
        </p>
      </header>
      <div className="bridges__body">
        {connectors.state.status === 'loading' ? (
          <p role="status">Checking connection choices and management access</p>
        ) : null}
        {connectors.state.status === 'error' ? (
          <p role="alert">
            {connectors.state.message}{' '}
            <Button onClick={connectors.reload}>Check access again</Button>
          </p>
        ) : null}
        <form
          onSubmit={(event) => {
            event.preventDefault();
            if (disabled || !reviewed || !configurationComplete(config) || !editor.canSave) return;
            const input = {
              config,
              owner:
                organization === undefined
                  ? { kind: 'person' as const, subject: viewerId }
                  : { kind: 'organization' as const, subject: organization },
            };
            let attempted = false;
            command.run('Create bridge', async (options) => {
              if (!attempted) {
                await verifyPlacementForSave(
                  client,
                  config,
                  organization,
                  undefined,
                  options.signal,
                );
                attempted = true;
              }
              const receipt = await client.bridges.create(input, options);
              options.signal?.throwIfAborted();
              onNavigate({ kind: 'bridge', bridgeId: receipt.bridge_id });
            });
          }}
        >
          <ConfigurationForm
            editor={editor}
            client={client}
            value={config}
            choices={choices}
            disabled={disabled}
            organization={organization}
            onChange={(value) => {
              setConfig(value);
              setReviewed(false);
            }}
          />
          <label className="bridges__check">
            <input
              type="checkbox"
              checked={reviewed}
              disabled={disabled}
              onChange={(event) => setReviewed(event.target.checked)}
            />
            I reviewed who owns the resources, who shares context and where replies are visible.
          </label>
          <p>
            Save the bridge, import its selected key, and validate the connection before enabling
            it.
          </p>
          <Button
            type="submit"
            variant="primary"
            disabled={disabled || !reviewed || !configurationComplete(config) || !editor.canSave}
          >
            {command.busy ? 'Saving bridge' : 'Save disabled bridge'}
          </Button>
        </form>
        {command.error === undefined ? null : <p role="alert">{command.error}</p>}
        {command.retryLabel === undefined || command.busy ? null : (
          <Button disabled={choices === undefined} onClick={command.retry}>
            Retry {command.retryLabel.toLowerCase()}
          </Button>
        )}
      </div>
    </section>
  );
}
