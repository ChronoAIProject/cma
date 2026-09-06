import { useState } from 'react';
import type { CmaWebClient } from '../api/client';
import type { AgentDetail } from '../api/types';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Modal } from '../ui/Modal';
import type { CreationClient, CreationProfile } from './client';
import { NewSandboxCard } from './NewSandboxCard';
import { NewWorkspaceCard } from './NewWorkspaceCard';
import { ProfilePicker } from './ProfilePicker';
import { WherePicker, type LocationChoice } from './WherePicker';
import { useCreationCommand } from './transaction';
import '../profiles/Profiles.css';
import './Create.css';

export function NewAgentCard({
  client,
  catalog,
  workspaceId,
  initial,
  onCreated,
  onChanged,
  onClose,
  canCreateSandbox = true,
}: {
  readonly client: CmaWebClient;
  readonly catalog: CreationClient;
  readonly workspaceId?: string;
  readonly initial?: CreationProfile;
  readonly onCreated: (agent: AgentDetail) => void;
  readonly onChanged: () => void;
  readonly onClose: () => void;
  readonly canCreateSandbox?: boolean;
}) {
  const [step, setStep] = useState<'where' | 'profile' | 'message'>('where');
  const [inline, setInline] = useState<'sandbox' | 'workspace'>();
  const [where, setWhere] = useState<LocationChoice>({ workspaceId });
  const [locationReady, setLocationReady] = useState(false);
  const [profile, setProfile] = useState(initial?.kind === 'agent' ? initial : undefined);
  const [title, setTitle] = useState('');
  const [firstMessage, setFirstMessage] = useState('');
  const command = useCreationCommand(onCreated);
  const titleInvalid =
    [...title.trim()].length > 64 ||
    [...title].some(
      (character) =>
        character.charCodeAt(0) < 32 ||
        (character.charCodeAt(0) >= 127 && character.charCodeAt(0) <= 159),
    );
  const messageInvalid = new TextEncoder().encode(firstMessage).length > 256 * 1024;
  const submit = () => {
    if (profile === undefined || where.workspaceId === undefined || titleInvalid || messageInvalid)
      return;
    const id = where.workspaceId;
    const input = {
      agentProfile: { id: profile.revision.profileId, revision: profile.revision.revision },
      title: title.trim() || undefined,
      firstMessage: firstMessage.trim() === '' ? undefined : firstMessage,
    };
    void command.submit((options) => client.createAgent(id, input, options));
  };
  if (inline === 'sandbox')
    return (
      <NewSandboxCard
        client={client}
        catalog={catalog}
        onClose={() => setInline(undefined)}
        onCreated={(sandbox) => {
          setWhere({ sandboxId: sandbox.id });
          setLocationReady(false);
          setInline(undefined);
          onChanged();
        }}
      />
    );
  if (inline === 'workspace')
    return (
      <NewWorkspaceCard
        client={client}
        catalog={catalog}
        sandboxId={where.sandboxId}
        canCreateSandbox={canCreateSandbox}
        onChanged={onChanged}
        onClose={() => setInline(undefined)}
        onCreated={(workspace) => {
          setWhere({ sandboxId: workspace.sandboxId, workspaceId: workspace.id });
          setLocationReady(false);
          setInline(undefined);
          onChanged();
        }}
      />
    );
  return (
    <Modal
      open
      title="New agent"
      onClose={onClose}
      dismissible={!command.locked}
      footer={
        <>
          <Button
            disabled={command.locked}
            onClick={() =>
              step === 'where' ? onClose() : setStep(step === 'message' ? 'profile' : 'where')
            }
          >
            Back
          </Button>
          {step === 'message' ? (
            <Button
              variant="primary"
              disabled={
                command.pending ||
                (!command.failure?.uncertain &&
                  (titleInvalid || messageInvalid || profile === undefined))
              }
              onClick={command.failure?.uncertain ? command.retry : submit}
            >
              {command.pending
                ? 'Creating'
                : command.failure?.uncertain
                  ? 'Retry creation'
                  : 'Create agent'}
            </Button>
          ) : (
            <Button
              variant="primary"
              disabled={step === 'where' ? !locationReady : profile === undefined}
              onClick={() => setStep(step === 'where' ? 'profile' : 'message')}
            >
              Continue
            </Button>
          )}
        </>
      }
    >
      <div className="create-form">
        <p>
          {step === 'where'
            ? 'Where'
            : step === 'profile'
              ? 'Which agent profile'
              : 'Title and first message'}
        </p>
        {step === 'where' ? (
          <WherePicker
            client={client}
            value={where}
            onChange={setWhere}
            onReady={setLocationReady}
            onNewSandbox={canCreateSandbox ? () => setInline('sandbox') : undefined}
            onNewWorkspace={() => setInline('workspace')}
          />
        ) : null}
        {step === 'profile' ? (
          <ProfilePicker
            client={catalog}
            kind="agent"
            value={profile}
            disabled={command.locked}
            onChange={(value) => setProfile(value?.kind === 'agent' ? value : undefined)}
          />
        ) : null}
        {step === 'message' ? (
          <>
            <div className="create-form__field">
              <label htmlFor="new-agent-title">Title (optional)</label>
              <Input
                id="new-agent-title"
                value={title}
                disabled={command.locked}
                aria-invalid={titleInvalid || undefined}
                onChange={(event) => setTitle(event.target.value)}
              />
              {titleInvalid ? (
                <p className="create-error">Use one line no longer than 64 characters.</p>
              ) : null}
            </div>
            <div className="create-form__field">
              <label htmlFor="new-agent-message">First message (optional)</label>
              <textarea
                className="profile-textarea"
                id="new-agent-message"
                rows={4}
                value={firstMessage}
                disabled={command.locked}
                aria-invalid={messageInvalid || undefined}
                onChange={(event) => setFirstMessage(event.target.value)}
              />
              {messageInvalid ? (
                <p className="create-error">Use a message no longer than 256 KB.</p>
              ) : null}
            </div>
          </>
        ) : null}
        {command.failure === undefined ? null : (
          <p className="create-error" role="alert">
            {command.failure.message}
          </p>
        )}
      </div>
    </Modal>
  );
}
