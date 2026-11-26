import { useMemo, useState } from 'react';
import type { ResourceViewsClient } from './client';
import { CommandKeys } from '../api/client';
import { useAuthenticatedSubject } from '../auth/context';
import { Modal } from '../ui/Modal';
import { useAuthorizedRead } from './useAuthorizedRead';
import { ShareScopeEditor } from './ShareScopeEditor';
import type { ShareTarget } from './sharing';
import './Sharing.css';

/**
 * Whether the viewer may manage sharing of this resource right now.
 *
 * The authority read is separate from the dialog so the entry point can be a
 * menu row: it appears only once current effective access grants
 * `manage_sharing`, and disappears again when that access expires.
 */
export function useSharingAllowed(
  client: ResourceViewsClient | undefined,
  target: Pick<ShareTarget, 'kind' | 'id'>,
): boolean {
  const { kind, id } = target;
  const read = useMemo(
    () =>
      client === undefined
        ? undefined
        : async (signal: AbortSignal) => {
            const access = await client.resourceAccess.effective(kind, id, { signal });
            return {
              value: access.capabilities.includes('manage_sharing'),
              revalidateAt: access.revalidate_at,
            };
          },
    [client, kind, id],
  );
  const authority = useAuthorizedRead(read);
  return authority.state.status === 'ready' && authority.state.value;
}

/**
 * The Sandbox sharing dialog.
 *
 * Callers key it by resource and keep it mounted while closed, so a new route
 * starts with fresh command keys and reopening on the same resource keeps them
 * for a same-key retry.
 */
export function SharingDialog({
  client,
  target,
  open,
  onClose,
}: {
  readonly client: ResourceViewsClient;
  readonly target: ShareTarget;
  readonly open: boolean;
  readonly onClose: () => void;
}) {
  const ownerId = useAuthenticatedSubject();
  const [keys] = useState(() => new CommandKeys());
  return (
    <Modal
      open={open}
      title="Sandbox sharing"
      onClose={onClose}
      description="Share access to this Sandbox and its Workspaces and Agents. Recipients cannot reshare it."
    >
      <p className="resource-sharing__scope">Sharing applies to the whole Sandbox.</p>
      {open ? (
        <ShareScopeEditor
          key={`${ownerId ?? ''}:${target.kind}:${target.id}`}
          ownerId={ownerId}
          client={client}
          target={target}
          keys={keys}
        />
      ) : null}
    </Modal>
  );
}
