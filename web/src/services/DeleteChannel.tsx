import { useChannelWriteGuard } from './useChannelWriteGuard';
import { useState } from 'react';
import { Button } from '../ui/Button';
import type { Channel } from './channelTypes';
import { segment, serviceRequest, ServiceError, type Denied } from './client';
export function DeleteChannel({
  channel,
  allowed = true,
  onDone,
  denied,
}: {
  readonly channel: Channel;
  readonly allowed?: boolean;
  readonly onDone: () => void;
  readonly denied: Denied;
}) {
  const canDelete = useChannelWriteGuard(allowed);
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  return (
    <div>
      {confirm ? (
        <>
          <p>
            Delete {channel.definition.name}? This disconnects its provider and stops incoming event
            delivery. Cleanup can be refused; the record is kept until deletion is confirmed.
          </p>
          <Button
            disabled={!allowed || busy}
            onClick={() => {
              if (!canDelete() || busy) return;
              setBusy(true);
              void serviceRequest(
                'event-gateway',
                `/v1/channels/${segment(channel.channel_id)}?expected_version=${channel.version}`,
                { method: 'DELETE' },
              )
                .then(onDone)
                .catch((e: unknown) => {
                  if (e instanceof ServiceError && [401, 403].includes(e.status)) denied(e.status);
                  setError(
                    e instanceof Error
                      ? e.message
                      : 'Deletion is unconfirmed. Refresh the channel before retrying.',
                  );
                })
                .finally(() => setBusy(false));
            }}
          >
            {busy ? 'Deleting…' : 'Confirm delete'}
          </Button>{' '}
          <Button disabled={busy} onClick={() => setConfirm(false)}>
            Cancel
          </Button>
        </>
      ) : (
        <Button
          disabled={!allowed}
          onClick={() => {
            if (canDelete()) setConfirm(true);
          }}
        >
          Delete channel
        </Button>
      )}
      {error ? <p role="alert">{error}</p> : null}
    </div>
  );
}
