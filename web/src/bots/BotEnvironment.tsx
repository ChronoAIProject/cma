import type { CreationClient, CreationProfile } from '../create/client';
import { ProfilePicker } from '../create/ProfilePicker';
import { TrustConfirmation } from '../create/TrustConfirmation';
import { ChoiceGroup } from '../profiles/ChoiceGroup';

export interface EnvironmentDraft {
  readonly mode: 'default' | 'custom';
  readonly sandbox?: CreationProfile;
  readonly workspace?: CreationProfile;
  readonly sandboxConfirmed?: boolean;
  readonly workspaceConfirmed?: boolean;
}

export function environmentReady(value: EnvironmentDraft) {
  return (
    value.mode === 'default' ||
    (value.sandbox?.kind === 'sandbox' &&
      value.workspace?.kind === 'workspace' &&
      (!value.sandbox.requiresConfirmation || value.sandboxConfirmed === true) &&
      (!value.workspace.requiresConfirmation || value.workspaceConfirmed === true))
  );
}

export function BotEnvironment({
  client,
  value,
  onChange,
}: {
  readonly client: CreationClient;
  readonly value: EnvironmentDraft;
  readonly onChange: (value: EnvironmentDraft) => void;
}) {
  return (
    <>
      <ChoiceGroup
        label="Environment profiles"
        value={value.mode}
        options={[
          { value: 'default', label: 'Use platform defaults' },
          { value: 'custom', label: 'Choose my published profiles' },
        ]}
        onChange={(mode) => onChange({ ...value, mode })}
      />
      {value.mode === 'default' ? (
        <p>
          The current default Sandbox and Workspace versions are pinned when you create this Bot.
        </p>
      ) : (
        <>
          <p>
            Choose your published environment to keep its installed software, service access,
            repository and setup commands.
          </p>
          <h3>Sandbox profile</h3>
          <ProfilePicker
            client={client}
            kind="sandbox"
            personalOnly
            value={value.sandbox}
            onChange={(sandbox) => onChange({ ...value, sandbox, sandboxConfirmed: false })}
          />
          {value.sandbox === undefined ? null : (
            <TrustConfirmation
              profile={value.sandbox}
              confirmed={value.sandboxConfirmed === true}
              onChange={(sandboxConfirmed) => onChange({ ...value, sandboxConfirmed })}
            />
          )}
          <h3>Workspace profile</h3>
          <ProfilePicker
            client={client}
            kind="workspace"
            personalOnly
            value={value.workspace}
            onChange={(workspace) => onChange({ ...value, workspace, workspaceConfirmed: false })}
          />
          {value.workspace === undefined ? null : (
            <TrustConfirmation
              profile={value.workspace}
              confirmed={value.workspaceConfirmed === true}
              onChange={(workspaceConfirmed) => onChange({ ...value, workspaceConfirmed })}
            />
          )}
        </>
      )}
    </>
  );
}
