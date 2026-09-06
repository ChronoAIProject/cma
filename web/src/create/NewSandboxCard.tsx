import { useState } from 'react';
import type { CmaWebClient } from '../api/client';
import type { SandboxDetail } from '../api/types';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Modal } from '../ui/Modal';
import type { CreationClient, CreationProfile } from './client';
import { ProfilePicker } from './ProfilePicker';
import { TrustConfirmation } from './TrustConfirmation';
import { useCreationCommand } from './transaction';
import '../profiles/Profiles.css';
import './Create.css';

export function NewSandboxCard({
  client,
  catalog,
  initial,
  onCreated,
  onClose,
}: {
  readonly client: CmaWebClient;
  readonly catalog: CreationClient;
  readonly initial?: CreationProfile;
  readonly onCreated: (sandbox: SandboxDetail) => void;
  readonly onClose: () => void;
}) {
  const [profile, setProfile] = useState(initial?.kind === 'sandbox' ? initial : undefined);
  const [confirmed, setConfirmed] = useState(false);
  const [name, setName] = useState('');
  const command = useCreationCommand(onCreated);
  const nameInvalid = new TextEncoder().encode(name.trim()).length > 64 || /[\r\n\0]/.test(name);
  const ready =
    profile !== undefined && (!profile.requiresConfirmation || confirmed) && !nameInvalid;
  const submit = () => {
    if (!ready || profile === undefined) return;
    const input = {
      sandboxProfile: { id: profile.revision.profileId, revision: profile.revision.revision },
      name: name.trim() || undefined,
      trustConfirmed: profile.requiresConfirmation ? confirmed : undefined,
      commandsDigest: profile.requiresConfirmation ? profile.revision.digest : undefined,
    };
    void command.submit((options) => client.createSandbox(input, options));
  };
  return (
    <Modal
      open
      title="New sandbox"
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
                : 'Create sandbox'}
          </Button>
        </>
      }
    >
      <form
        className="create-form"
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
      >
        <ProfilePicker
          client={catalog}
          kind="sandbox"
          value={profile}
          disabled={command.locked}
          onChange={(value) => {
            setProfile(value?.kind === 'sandbox' ? value : undefined);
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
        <div className="create-form__field">
          <label htmlFor="new-sandbox-name">Name (optional)</label>
          <Input
            id="new-sandbox-name"
            value={name}
            disabled={command.locked}
            aria-invalid={nameInvalid || undefined}
            onChange={(event) => setName(event.target.value)}
          />
          {nameInvalid ? (
            <p className="create-error">Use one line no longer than 64 bytes.</p>
          ) : null}
        </div>
        {command.failure === undefined ? null : (
          <p className="create-error" role="alert">
            {command.failure.message}
          </p>
        )}
      </form>
    </Modal>
  );
}
