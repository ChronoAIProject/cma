import { useMemo, useState } from 'react';
import type { BridgeConfigurationDetail, BridgeInvokeReadiness } from '@chronoai/cma-sdk';
import type { BridgeViewsClient } from './client';
import { useAuthorizedRead } from '../resources/useAuthorizedRead';
import { Button } from '../ui/Button';
import { InlineConfirm } from '../ui/InlineConfirm';
import type { BridgeCommand } from './commands';

export function BridgeConsent({
  client,
  detail,
  readiness,
  allowed,
  onChanged,
  command,
}: {
  readonly client: BridgeViewsClient;
  readonly detail: BridgeConfigurationDetail;
  readonly readiness?: BridgeInvokeReadiness;
  readonly allowed: boolean;
  readonly onChanged: () => void;
  readonly command: BridgeCommand;
}) {
  const [reviewed, setReviewed] = useState(false);
  const [revoke, setRevoke] = useState<string>();
  const [cursors, setCursors] = useState<readonly (string | undefined)[]>([undefined]);
  const cursor = cursors.at(-1);
  const id = detail.bridge.id;
  const read = useMemo(
    () =>
      !allowed
        ? undefined
        : async (signal: AbortSignal) => ({
            value: await client.bridges.listInvokeGrants(id, { signal, cursor, limit: 50 }),
          }),
    [client, id, allowed, cursor],
  );
  const history = useAuthorizedRead(read);
  const page = history.state.status === 'ready' ? history.state.value : undefined;
  const placement = detail.config.placement;
  const current = readiness?.candidate;
  const changed = () => {
    setReviewed(false);
    history.reload();
    onChanged();
  };
  return (
    <section className="bridges__fields" aria-labelledby="bridge-consent-title">
      <h2 id="bridge-consent-title">Retained Agent permission</h2>
      {placement.kind !== 'existing_agent' ? (
        <p>
          This placement creates resources for each conversation. Permission to invoke a retained
          Agent is not required.
        </p>
      ) : (
        <>
          <p>
            {current === undefined || current === null
              ? 'This bridge needs explicit permission to use the selected Agent’s existing context.'
              : 'Permission is recorded for the selected Agent and saved settings.'}
          </p>
          <p>
            Allowed external senders can cause this Agent to use its history, files, tools and
            connected services. Its replies can reveal that context to the original chat.
          </p>
          <label className="bridges__check">
            <input
              type="checkbox"
              checked={reviewed}
              disabled={!allowed || command.locked || current != null}
              onChange={(event) => setReviewed(event.target.checked)}
            />
            I authorize this bridge’s allowed senders to use the retained Agent’s existing context.
          </label>
          <Button
            disabled={
              !allowed ||
              readiness === undefined ||
              command.locked ||
              !reviewed ||
              current != null ||
              detail.bridge.state.kind === 'retired'
            }
            onClick={() =>
              command.run('Grant Agent permission', async (options) => {
                await client.bridges.issueInvokeGrant(
                  id,
                  {
                    expected_bridge_revision: detail.bridge.revision,
                    target: placement.target,
                    shared_context: 'use_retained_agent_context',
                  },
                  options,
                );
                options.signal?.throwIfAborted();
                changed();
              })
            }
          >
            Grant Agent permission
          </Button>
          <p>
            Permission status is checked again when enabling and processing work. This view does not
            prove message execution or reply delivery.
          </p>
        </>
      )}
      {page === undefined ? null : (
        <>
          <ul className="bridges__rows" aria-label="Retained Agent permissions">
            {page.items.map((grant) => (
              <li key={grant.id}>
                <span>Saved settings {grant.pin.config_revision}</span>
                <span>
                  {grant.state.kind === 'active' ? 'Active permission' : 'Revoked permission'}
                </span>
                <time dateTime={grant.issued_at}>
                  {new Date(grant.issued_at).toLocaleDateString()}
                </time>
                {grant.state.kind !== 'active' ? null : revoke === grant.id ? (
                  <InlineConfirm
                    question="Revoke permission to use this Agent through the bridge?"
                    confirmLabel="Revoke permission"
                    busy={!allowed || command.busy}
                    onCancel={() => setRevoke(undefined)}
                    onConfirm={() =>
                      command.run('Revoke Agent permission', async (options) => {
                        await client.bridges.revokeInvokeGrant(
                          grant.id,
                          {
                            expected_bridge_revision: detail.bridge.revision,
                            expected_revision: grant.revision,
                          },
                          options,
                        );
                        options.signal?.throwIfAborted();
                        setRevoke(undefined);
                        changed();
                      })
                    }
                  />
                ) : (
                  <Button disabled={!allowed || command.locked} onClick={() => setRevoke(grant.id)}>
                    Revoke permission
                  </Button>
                )}
              </li>
            ))}
          </ul>
          {page.next_cursor !== null || cursors.length > 1 ? (
            <nav className="bridges__actions" aria-label="Agent permission pages">
              <Button
                disabled={cursors.length === 1}
                onClick={() => setCursors((items) => items.slice(0, -1))}
              >
                Previous permissions
              </Button>
              <Button
                disabled={page.next_cursor === null}
                onClick={() => setCursors((items) => [...items, page.next_cursor ?? undefined])}
              >
                More permissions
              </Button>
            </nav>
          ) : null}
        </>
      )}
      {history.state.status === 'error' ? (
        <p role="status">
          Permission history is unavailable.{' '}
          <Button onClick={history.reload}>Check permissions</Button>
        </p>
      ) : null}
    </section>
  );
}
