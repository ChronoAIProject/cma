import { useMemo, useState } from 'react';
import type { ResourceViewsClient } from './client';
import { useAuthorizedRead } from './useAuthorizedRead';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import { AgentArtifacts } from './AgentArtifacts';
import { PublishArtifact, ArtifactPublicationRecovery } from './PublishArtifact';
import type { ArtifactPublicationScope } from './artifactRecovery';

interface Props {
  readonly client: ResourceViewsClient;
  readonly id: string;
  readonly publicationScope?: ArtifactPublicationScope;
}
export function ArtifactButton(props: Props) {
  const [scope, setScope] = useState({ client: props.client, id: props.id, revision: 0 });
  if (scope.client !== props.client || scope.id !== props.id)
    setScope({ client: props.client, id: props.id, revision: scope.revision + 1 });
  return <ArtifactEntry key={scope.revision} {...props} />;
}
function ArtifactEntry({ client, id, publicationScope }: Props) {
  const [openedFor, setOpenedFor] = useState<ResourceViewsClient>();
  const [revision, setRevision] = useState(0);
  const [localRecovery] = useState(() => new ArtifactPublicationRecovery());
  const recovery = publicationScope?.for(client, id) ?? localRecovery;
  const read = useMemo(
    () => async (signal: AbortSignal) => {
      const access = await client.resourceAccess.effective('agent', id, { signal });
      return { value: access, revalidateAt: access.revalidate_at };
    },
    [client, id],
  );
  const current = useAuthorizedRead(read);
  const access = current.state.status === 'ready' ? current.state.value : undefined;
  const allowed = access?.capabilities.includes('view_artifacts') === true;
  const canPublish = access?.capabilities.includes('manage_sharing') === true;
  const [lastAccess, setLastAccess] = useState({ view: false, publish: false });
  if (access !== undefined && (lastAccess.view !== allowed || lastAccess.publish !== canPublish))
    setLastAccess({ view: allowed, publish: canPublish });
  const checking = current.state.status === 'loading' || current.refreshing;
  const retainView = allowed || (checking && lastAccess.view);
  const retainPublication = canPublish || (checking && lastAccess.publish);
  return (
    <>
      {allowed ? (
        <Button disabled={checking} onClick={() => setOpenedFor(client)}>
          Artifacts
        </Button>
      ) : null}
      <Modal
        open={openedFor === client}
        title="Agent artifacts"
        onClose={() => setOpenedFor(undefined)}
      >
        {retainView ? (
          <>
            <AgentArtifacts
              key={`${id}:${revision}`}
              client={client}
              id={id}
              onAccessChanged={current.reload}
              enabled={allowed && !checking}
            />
            {retainPublication ? (
              <PublishArtifact
                recovery={recovery}
                key={id}
                client={client}
                id={id}
                onPublished={() => setRevision((value) => value + 1)}
                onAccessChanged={current.reload}
                enabled={allowed && canPublish && !checking}
              />
            ) : null}
          </>
        ) : (
          <p role="alert">
            Artifact access is unavailable. Refresh the Agent to check your current access.
          </p>
        )}
      </Modal>
    </>
  );
}
