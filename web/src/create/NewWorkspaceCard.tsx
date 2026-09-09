import { Presence } from '../ui/Presence';
import { useState } from 'react';
import type { CmaWebClient } from '../api/client';
import type { WorkspaceDetail } from '../api/types';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import type { CreationClient, CreationProfile } from './client';
import { ProfilePicker } from './ProfilePicker';
import { TrustConfirmation } from './TrustConfirmation';
import { WherePicker, type LocationChoice } from './WherePicker';
import { useCreationCommand } from './transaction';
import { NewSandboxCard } from './NewSandboxCard';
import '../profiles/Profiles.css';
import './Create.css';

export function NewWorkspaceCard({
  client,
  catalog,
  sandboxId,
  initial,
  onCreated,
  onClose,
  canCreateSandbox = true,
  onChanged,
}: {
  readonly client: CmaWebClient;
  readonly catalog: CreationClient;
  readonly sandboxId?: string;
  readonly initial?: CreationProfile;
  readonly onCreated: (workspace: WorkspaceDetail) => void;
  readonly onClose: () => void;
  readonly canCreateSandbox?: boolean;
  readonly onChanged?: () => void;
}) {
  const [where, setWhere] = useState<LocationChoice>({ sandboxId });
  const [newSandbox, setNewSandbox] = useState(false);
  const [locationReady, setLocationReady] = useState(false);
  const [profile, setProfile] = useState(initial?.kind === 'workspace' ? initial : undefined);
  const [confirmed, setConfirmed] = useState(false);
  const command = useCreationCommand(onCreated);
  const ready =
    profile !== undefined && locationReady && (!profile.requiresConfirmation || confirmed);
  const submit = () => {
    if (!ready || profile === undefined || where.sandboxId === undefined) return;
    const id = where.sandboxId;
    const input = {
      workspaceProfile: { id: profile.revision.profileId, revision: profile.revision.revision },
      trustConfirmed: profile.requiresConfirmation ? confirmed : undefined,
      commandsDigest: profile.requiresConfirmation ? profile.revision.digest : undefined,
    };
    void command.submit((options) => client.createWorkspace(id, input, options));
  };
  if (newSandbox)
    return (
      <NewSandboxCard
        client={client}
        catalog={catalog}
        onClose={() => setNewSandbox(false)}
        onCreated={(sandbox) => {
          setWhere({ sandboxId: sandbox.id });
          setLocationReady(false);
          setNewSandbox(false);
          onChanged?.();
        }}
      />
    );
  return (
    <Modal
      open
      title="New workspace"
      onClose={onClose}
      dismissible={!command.locked}
      footer={
        <>
          <Button disabled={command.locked} onClick={onClose}>
            Back
          </Button>
          <Button
            variant="primary"
            disabled={command.pending || (!command.failure?.uncertain && !ready)}
            onClick={command.failure?.uncertain ? command.retry : submit}
          >
            {command.pending
              ? 'Creating'
              : command.failure?.uncertain
                ? 'Retry creation'
                : 'Create workspace'}
          </Button>
        </>
      }
    >
      <div className="create-form">
        <WherePicker
          client={client}
          value={where}
          onChange={setWhere}
          onReady={setLocationReady}
          workspace={false}
          disabled={command.locked}
          onNewSandbox={canCreateSandbox ? () => setNewSandbox(true) : undefined}
        />
        <ProfilePicker
          client={catalog}
          kind="workspace"
          value={profile}
          disabled={command.locked}
          onChange={(value) => {
            setProfile(value?.kind === 'workspace' ? value : undefined);
            setConfirmed(false);
          }}
        />
        {profile === undefined ? null : (
          <TrustConfirmation
            profile={profile}
            confirmed={confirmed}
            onChange={setConfirmed}
            disabled={command.locked}
          />
        )}
        <Presence>
          {command.failure === undefined ? null : (
            <p className="create-error" role="alert">
              {command.failure.message}
            </p>
          )}
        </Presence>
      </div>
    </Modal>
  );
}
