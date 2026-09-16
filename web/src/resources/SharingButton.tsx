import { useMemo, useState } from 'react';
import type { ResourceViewsClient } from './client';
import { CommandKeys } from '../api/client';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import { Picker } from '../ui/Picker';
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
  const [scope, setScope] = useState<'agent' | 'sandbox'>(target.kind);
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
  const selected = scope === 'sandbox' && sandbox !== undefined ? sandbox : target;
  return (
    <>
      {allowed ? (
        <Button
          onClick={() => {
            setScope(target.kind);
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
        {target.kind === 'agent' && sandbox !== undefined ? (
          <div className="resource-sharing__scope">
            <Picker
              label="Share scope"
              value={scope}
              options={[
                { value: 'agent', label: 'This Agent' },
                { value: 'sandbox', label: 'Whole Sandbox' },
              ]}
              onChange={setScope}
            />
          </div>
        ) : null}
        {openedFor === client ? (
          <ShareScopeEditor
            key={`${selected.kind}:${selected.id}`}
            client={client}
            target={selected}
            keys={keys}
          />
        ) : null}
      </Modal>
    </>
  );
}
