import { useState } from 'react';
import { useCmaClient } from '../api/context';
import { useAuthenticatedSubject } from '../auth/context';
import { AgentShareLinksDialog } from './AgentShareLinksDialog';
export { AgentShareLinksDialog } from './AgentShareLinksDialog';

/** This control is mounted only in an owner's authorized Agent Details. */
export function AgentShareLinks({
  agentId,
  active = true,
}: {
  readonly agentId: string;
  readonly active?: boolean;
}) {
  const client = useCmaClient().resourceViews;
  const ownerId = useAuthenticatedSubject();
  const [open, setOpen] = useState(false);
  if (!client) return null;
  return (
    <section className="agent-details__band">
      <h2>Sharing</h2>
      <p>Share this Agent conversation and files in its Sandbox home.</p>
      <button
        type="button"
        className="agent-details__link"
        disabled={!active}
        onClick={() => setOpen(true)}
      >
        Manage share links
      </button>
      <AgentShareLinksDialog
        ownerId={ownerId}
        client={client}
        agentId={agentId}
        open={open && active}
        onClose={() => setOpen(false)}
      />
    </section>
  );
}
