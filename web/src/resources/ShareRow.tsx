import { useState } from 'react';
import type { ResourceShare } from '@chronoai/cma-sdk';
import type { ResourceViewsClient } from './client';
import { Button } from '../ui/Button';
import { Picker } from '../ui/Picker';
import { InlineConfirm } from '../ui/InlineConfirm';
import { shareExposure, shareModes, type ShareTarget } from './sharing';
import type { ShareCommand } from './ShareScopeEditor';

export function ShareRow({
  share,
  name,
  target,
  client,
  busy,
  onCommand,
}: {
  readonly share: ResourceShare;
  readonly name: string;
  readonly target: ShareTarget;
  readonly client: ResourceViewsClient;
  readonly busy: boolean;
  readonly onCommand: (command: ShareCommand) => Promise<void>;
}) {
  const [mode, setMode] = useState(share.mode);
  const [revoke, setRevoke] = useState(false);
  return (
    <li>
      <h4>{name}</h4>
      <p>Current access: {shareModes.find((item) => item.value === share.mode)?.label}</p>
      <Picker
        label={`Access for ${name}`}
        value={mode}
        options={shareModes}
        disabled={busy || revoke}
        onChange={setMode}
      />
      {mode === share.mode ? null : <p>{shareExposure(mode)}</p>}
      {revoke ? (
        <InlineConfirm
          question={`Revoke this direct grant for ${name}? Other grants may still provide access.`}
          confirmLabel="Revoke grant"
          busy={busy}
          cancelDisabled={busy}
          onCancel={() => setRevoke(false)}
          onConfirm={() =>
            void onCommand({
              key: `revoke:${share.id}:${share.revision}`,
              expected: {
                organization: share.target_org_id,
                mode: share.mode,
                id: share.id,
                revision: share.revision + 1,
                revoked: true,
              },
              send: (signal, idempotencyKey) =>
                client.resourceAccess.revokeShare(
                  target.kind,
                  target.id,
                  share.id,
                  share.revision,
                  { signal, idempotencyKey },
                ),
            })
          }
        />
      ) : (
        <div className="resource-sharing__actions">
          <Button
            disabled={busy || mode === share.mode}
            onClick={() =>
              void onCommand({
                key: `update:${share.id}:${share.revision}:${mode}`,
                expected: {
                  organization: share.target_org_id,
                  mode,
                  id: share.id,
                  revision: share.revision + 1,
                  revoked: false,
                },
                send: (signal, idempotencyKey) =>
                  client.resourceAccess.updateShare(
                    target.kind,
                    target.id,
                    share.id,
                    { expected_revision: share.revision, mode },
                    { signal, idempotencyKey },
                  ),
              })
            }
          >
            Save access
          </Button>
          <Button disabled={busy} onClick={() => setRevoke(true)}>
            Revoke
          </Button>
        </div>
      )}
    </li>
  );
}
