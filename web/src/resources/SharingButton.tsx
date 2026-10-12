import { useMemo, useState } from 'react';
import type { ResourceViewsClient } from './client';
import { CommandKeys } from '../api/client';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import { useAuthorizedRead } from './useAuthorizedRead';
import { ShareScopeEditor } from './ShareScopeEditor';
import type { ShareTarget } from './sharing';
import './Sharing.css';

export function SharingButton({
  client,
  target,
  sandbox,
}: {
  readonly client: ResourceViewsClient;
  readonly target: ShareTarget;
  readonly sandbox?: ShareTarget;
}) {
  // The button and dialog belong to one resource/client. A new route starts closed.
  return (
    <SharingEntry
      key={`${target.kind}:${target.id}`}
      client={client}
      target={target}
      sandbox={sandbox}
    />
  );
}

function SharingEntry({
  client,
  target,
  sandbox,
}: {
  readonly client: ResourceViewsClient;
  readonly target: ShareTarget;
  readonly sandbox?: ShareTarget;
}) {
  const [openedFor, setOpenedFor] = useState<ResourceViewsClient>();
  const [keys] = useState(() => new CommandKeys());
  const selectedTarget = sandbox ?? target;
  const { kind, id } = selectedTarget;
  const read = useMemo(
    () => async (signal: AbortSignal) => {
      const access = await client.resourceAccess.effective(kind, id, { signal });
      return {
        value: access.capabilities.includes('manage_sharing'),
        revalidateAt: access.revalidate_at,
      };
    },
    [client, kind, id],
  );
  const authority = useAuthorizedRead(read);
  const allowed = authority.state.status === 'ready' && authority.state.value;
  return (
    <>
      {allowed ? (
        <Button
          onClick={() => {
            setOpenedFor(client);
          }}
        >
          Sharing
        </Button>
      ) : null}
      <Modal
        open={openedFor === client}
        title="Resource sharing"
        onClose={() => setOpenedFor(undefined)}
        description="Sharing preserves the owner, creation origin and conversation. Recipients cannot reshare your resource."
      >
        {selectedTarget.kind === 'sandbox' ? (
          <p className="resource-sharing__scope">Sharing applies to the whole Sandbox.</p>
        ) : null}
        {openedFor === client ? (
          <ShareScopeEditor
            key={`${selectedTarget.kind}:${selectedTarget.id}`}
            client={client}
            target={selectedTarget}
            keys={keys}
          />
        ) : null}
      </Modal>
    </>
  );
}
