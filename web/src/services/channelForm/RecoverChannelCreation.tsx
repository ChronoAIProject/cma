import { useState } from 'react';
import { Button } from '../../ui/Button';
import { classifyChannelLifecycle } from '../channelLifecycle';
import { hasChannelAddress, type Channel, type ChannelListItem } from '../channelTypes';
import { segment, ServiceError, serviceRequest, type Denied } from '../client';
import { clearNewChannelDraft } from './newChannelDraft';

export function RecoverChannelCreation({
  owner,
  record,
  denied,
  onFound,
}: {
  readonly owner: string;
  readonly record: string;
  readonly denied: Denied;
  readonly onFound: (channel: Channel) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string>();
  async function check() {
    if (busy) return;
    setBusy(true);
    setMessage(undefined);
    try {
      const result = await serviceRequest<ChannelListItem>(
        'event-gateway',
        `/v1/channel-records/${segment(record)}`,
      );
      if (
        result?.record_id === record &&
        hasChannelAddress(result) &&
        result.definition &&
        Number.isSafeInteger(result.version) &&
        result.version >= 0
      ) {
        const kind = classifyChannelLifecycle(result.definition).kind;
        if (kind === 'complete' || kind === 'incomplete') {
          clearNewChannelDraft(owner, record);
          onFound(result);
          return;
        }
      }
      setMessage('Could not confirm the original channel. Check again before creating another.');
    } catch (error) {
      if (error instanceof ServiceError && [401, 403].includes(error.status)) denied(error.status);
      setMessage(
        error instanceof ServiceError && error.status === 404
          ? 'The original channel is not visible yet. Check again before creating another.'
          : 'Could not check the original channel. Try again.',
      );
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="channel-node-editor">
      <p>
        Creation was submitted before reload. Check the original channel before starting another.
      </p>
      <div className="service-actions">
        <Button variant="primary" disabled={busy} onClick={() => void check()}>
          Check original channel
        </Button>
      </div>
      {message ? <p role="status">{message}</p> : null}
    </div>
  );
}
