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
}: {
  readonly client: ResourceViewsClient;
  readonly target: ShareTarget;
}) {
  // The button and dialog belong to one resource/client. A new route starts closed.
  return <SharingEntry key={`${target.kind}:${target.id}`} client={client} target={target} />;
}

function SharingEntry({
  client,
  target,
}: {
  readonly client: ResourceViewsClient;
  readonly target: ShareTarget;
}) {
  const [openedFor, setOpenedFor] = useState<ResourceViewsClient>();
  const [keys] = useState(() => new CommandKeys());
  const { kind, id } = target;
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
        title="Sandbox sharing"
        onClose={() => setOpenedFor(undefined)}
        description="Share access to this Sandbox and its Workspaces and Agents. Recipients cannot reshare it."
      >
        <p className="resource-sharing__scope">Sharing applies to the whole Sandbox.</p>
        {openedFor === client ? (
          <ShareScopeEditor
            key={`${target.kind}:${target.id}`}
            client={client}
            target={target}
            keys={keys}
          />
        ) : null}
      </Modal>
    </>
  );
}
