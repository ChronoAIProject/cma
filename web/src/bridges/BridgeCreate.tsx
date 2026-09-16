import { useMemo, useState } from 'react';
import type { BridgePageProps } from './BridgePage';
import { useAuthorizedRead } from '../resources/useAuthorizedRead';
import { ResourceLink } from '../resources/ResourceLink';
import { Button } from '../ui/Button';
import { ConfigurationForm } from './ConfigurationForm';
import { usePlacementEditor } from './usePlacementEditor';
import { verifyPlacementForSave } from './placementSave';
import { initialConfiguration, configurationComplete } from './configuration';
import { bridgeFailure } from './commands';
import { useBridgeCreation } from './useBridgeCreation';
import { BridgeCreationCallback } from './BridgeCreationCallback';

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
  const creation = useBridgeCreation(client, viewerId, organization);
  const disabled = choices === undefined || creation.busy || creation.pending || !creation.ready;
  const navigateToReceipt = (receipt: Awaited<ReturnType<typeof creation.submit>>) => {
    if (receipt !== undefined) onNavigate({ kind: 'bridge', bridgeId: receipt.bridge_id });
  };
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
        <BridgeCreationCallback creation={creation} />
        <Button onClick={connectors.reload} disabled={creation.busy}>
          Refresh connection choices
        </Button>
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
            void creation
              .submit(input, (signal) =>
                verifyPlacementForSave(client, config, organization, undefined, signal),
              )
              .then(navigateToReceipt);
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
            {creation.busy ? 'Saving bridge' : 'Save disabled bridge'}
          </Button>
        </form>
        {creation.error === undefined ? null : <p role="alert">{creation.error}</p>}
        {!creation.pending || creation.busy ? null : (
          <div>
            <p>A save may already have completed. Recover its result before changing this setup.</p>
            <Button
              disabled={!creation.ready}
              onClick={() => {
                void creation.submit().then(navigateToReceipt);
              }}
            >
              Recover saved bridge
            </Button>
          </div>
        )}
      </div>
    </section>
  );
}
