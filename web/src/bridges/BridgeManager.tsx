import { useMemo, useState } from 'react';
import type {
  BridgeConfiguration,
  BridgeAcceptedWork,
  BridgeConfigurationDetail,
} from '@chronoai/cma-sdk';
import type { BridgeViewsClient } from './client';
import { useAuthorizedRead } from '../resources/useAuthorizedRead';
import { Button } from '../ui/Button';
import { Picker } from '../ui/Picker';
import { ConfigurationForm } from './ConfigurationForm';
import { usePlacementEditor } from './usePlacementEditor';
import { verifyPlacementForSave } from './placementSave';
import { BridgeCredential } from './BridgeCredential';
import { BridgeLifecycle } from './BridgeLifecycle';
import { BridgeConsent } from './BridgeConsent';
import { configurationComplete } from './configuration';
import { bridgeFailure, useBridgeCommand } from './commands';

export function BridgeManager({
  client,
  id,
  allowed,
  onChanged,
}: {
  readonly client: BridgeViewsClient;
  readonly id: string;
  readonly allowed: boolean;
  readonly onChanged: () => void;
}) {
  const [draft, setDraft] = useState<{
    readonly revision: number;
    readonly config: BridgeConfiguration;
  }>();
  const [work, setWork] = useState<BridgeAcceptedWork>('suspend_original');
  const [reviewed, setReviewed] = useState(false);
  const [validated, setValidated] = useState<number>();
  const [notice, setNotice] = useState<string>();
  const read = useMemo(
    () =>
      !allowed
        ? undefined
        : async (signal: AbortSignal) => {
            try {
              return { value: await client.bridges.configuration(id, { signal }) };
            } catch (error) {
              throw new Error(bridgeFailure(error));
            }
          },
    [client, id, allowed],
  );
  const current = useAuthorizedRead(read);
  const detail: BridgeConfigurationDetail | undefined =
    current.state.status === 'ready' ? current.state.value : undefined;
  const organization =
    detail?.bridge.owner.kind === 'organization' ? detail.bridge.owner.subject : undefined;
  const hasDetail = detail !== undefined;
  const revision = detail?.bridge.revision;
  const consentRead = useMemo(
    () =>
      !allowed || revision === undefined
        ? undefined
        : async (signal: AbortSignal) => {
            const value = await client.bridges.invokeReadiness(id, { signal });
            if (value.revision !== revision)
              throw new Error('Bridge settings changed. Refresh current permission.');
            return { value };
          },
    [client, id, allowed, revision],
  );
  const consent = useAuthorizedRead(consentRead);
  const readiness = consent.state.status === 'ready' ? consent.state.value : undefined;
  const readChoices = useMemo(
    () =>
      !allowed || !hasDetail
        ? undefined
        : async (signal: AbortSignal) => {
            try {
              return { value: await client.bridges.choices({ organization, signal }) };
            } catch (error) {
              throw new Error(bridgeFailure(error));
            }
          },
    [client, allowed, hasDetail, organization],
  );
  const connectors = useAuthorizedRead(readChoices);
  const choices = connectors.state.status === 'ready' ? connectors.state.value : undefined;
  const command = useBridgeCommand(allowed && detail !== undefined);
  const changed = () => {
    setValidated(undefined);
    current.reload();
    onChanged();
  };
  const config = draft?.config ?? detail?.config;
  const editor = usePlacementEditor(
    client,
    allowed && detail !== undefined ? config : undefined,
    organization,
    detail?.config.placement,
  );
  const stale =
    draft !== undefined && detail !== undefined && draft.revision !== detail.bridge.revision;
  if (!allowed) return null;
  return (
    <div>
      {current.state.status === 'loading' ? (
        <p role="status">Checking current bridge management access</p>
      ) : null}
      {current.state.status === 'error' ? (
        <p role="alert">
          {current.state.message} <Button onClick={current.reload}>Check management access</Button>
        </p>
      ) : null}
      {detail === undefined ? null : (
        <>
          <section className="bridges__fields" aria-labelledby="bridge-connection-title">
            <h2 id="bridge-connection-title">Connection setup</h2>
            <p>In NyxID, set the selected key’s callback to this exact address:</p>
            <code className="bridges__callback">{detail.callback_url}</code>
            <p>
              Use a dedicated matching key if another active route already uses its callback. Save
              the change in NyxID, then validate it here.
            </p>
            <Button
              disabled={
                command.locked || draft !== undefined || detail.bridge.state.kind === 'retired'
              }
              onClick={() =>
                command.run('Validate connection', async (options) => {
                  await client.bridges.validate(id, detail.bridge.revision, options);
                  options.signal?.throwIfAborted();
                  setValidated(detail.bridge.revision);
                  setNotice(
                    'The selected connection is verified for these saved settings. Message execution and delivery require their own readiness checks.',
                  );
                })
              }
            >
              Validate connection
            </Button>
          </section>
          <BridgeCredential
            key={`${detail.config.binding.key_id}:${detail.credential.revision}`}
            client={client}
            detail={detail}
            allowed={!command.locked}
            onChanged={changed}
            sharedCommand={command}
          />
          <BridgeConsent
            key={`${id}:${detail.bridge.config_revision}`}
            client={client}
            detail={detail}
            readiness={readiness}
            allowed={!command.locked && draft === undefined}
            onChanged={consent.reload}
            command={command}
          />
          {consent.state.status === 'error' ? (
            <p role="alert">
              Current Agent permission could not be verified.{' '}
              <Button onClick={consent.reload}>Check Agent permission</Button>
            </p>
          ) : null}
        </>
      )}
      {config === undefined || detail === undefined ? null : (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            if (
              command.locked ||
              !reviewed ||
              stale ||
              choices === undefined ||
              !configurationComplete(config) ||
              !editor.canSave
            )
              return;
            const body = { expected_revision: detail.bridge.revision, config, previous_work: work };
            let attempted = false;
            command.run('Save settings', async (options) => {
              if (!attempted) {
                await verifyPlacementForSave(
                  client,
                  config,
                  organization,
                  detail.config.placement,
                  options.signal,
                );
                attempted = true;
              }
              await client.bridges.update(id, body, options);
              options.signal?.throwIfAborted();
              setDraft(undefined);
              setReviewed(false);
              setNotice('Bridge settings saved. Validate the current connection before enabling.');
              changed();
            });
          }}
        >
          <ConfigurationForm
            editor={editor}
            client={client}
            value={config}
            choices={choices}
            existing
            disabled={
              command.locked || choices === undefined || detail.bridge.state.kind === 'retired'
            }
            organization={organization}
            onChange={(value) => {
              setDraft({ revision: draft?.revision ?? detail.bridge.revision, config: value });
              setReviewed(false);
              setValidated(undefined);
            }}
          />
          {connectors.state.status === 'error' ? (
            <p role="alert">
              {connectors.state.message}{' '}
              <Button onClick={connectors.reload}>Reload connection choices</Button>
            </p>
          ) : null}
          <Picker
            label="Accepted work after settings change"
            value={work}
            disabled={command.locked}
            options={[
              { value: 'suspend_original', label: 'Suspend work accepted with prior settings' },
              { value: 'drain_original', label: 'Finish accepted work with its original settings' },
            ]}
            onChange={setWork}
          />
          <label className="bridges__check">
            <input
              type="checkbox"
              disabled={command.locked}
              checked={reviewed}
              onChange={(event) => setReviewed(event.target.checked)}
            />
            I reviewed resource ownership, shared context and reply visibility for these settings.
          </label>
          {stale ? (
            <p role="alert">
              The bridge changed while you were editing. Your draft is retained. Reload the saved
              settings before applying a new change.
            </p>
          ) : null}
          <div className="bridges__actions">
            <Button
              type="submit"
              variant="primary"
              disabled={
                command.locked ||
                choices === undefined ||
                !reviewed ||
                stale ||
                draft === undefined ||
                !configurationComplete(config) ||
                !editor.canSave
              }
            >
              Save settings
            </Button>
            <Button
              disabled={command.locked || draft === undefined}
              onClick={() => {
                setDraft(undefined);
                setReviewed(false);
                current.reload();
              }}
            >
              Reload saved settings
            </Button>
          </div>
        </form>
      )}
      {detail === undefined ? null : (
        <BridgeLifecycle
          bridge={detail.bridge}
          disabled={command.locked || draft !== undefined}
          connectionVerified={
            validated === detail.bridge.revision &&
            readiness !== undefined &&
            readiness.status !== 'consent_required'
          }
          onControl={(control) =>
            command.run(`${control.kind} bridge`, async (options) => {
              await client.bridges.control(
                id,
                { expected_revision: detail.bridge.revision, command: control },
                options,
              );
              options.signal?.throwIfAborted();
              setNotice('Bridge control confirmed.');
              changed();
            })
          }
        />
      )}
      {notice === undefined ? null : <p role="status">{notice}</p>}
      {command.error === undefined ? null : <p role="alert">{command.error}</p>}
      {command.retryLabel === undefined || command.busy ? null : (
        <Button disabled={detail === undefined} onClick={command.retry}>
          Retry {command.retryLabel.toLowerCase()}
        </Button>
      )}
    </div>
  );
}
