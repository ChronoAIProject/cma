import type { SharedPlacement } from './placementAccess';
import type { PlacementEditor } from './usePlacementEditor';
import type { BridgeViewsClient } from './client';
import { SharedParentPicker } from './SharedParentPicker';
import { ProfileSelection } from './ProfileSelection';
import { Input } from '../ui/Input';
import { Button } from '../ui/Button';

export function SharedPlacementFields({
  client,
  value,
  disabled,
  organization,
  editor,
  onChange,
}: {
  readonly client: BridgeViewsClient;
  readonly value: SharedPlacement;
  readonly disabled: boolean;
  readonly organization?: string;
  readonly editor: PlacementEditor;
  readonly onChange: (value: SharedPlacement) => void;
}) {
  const unavailable = disabled || editor.reason !== undefined;
  return (
    <div>
      <p>
        All participants share the Sandbox’s filesystem, processes and credentials. Separate
        Worktrees and Agent homes keep conversations distinct; they do not isolate access to sibling
        work.
      </p>
      {editor.reason === undefined ? null : (
        <p role="status">{editor.reason} Saved placement settings remain visible.</p>
      )}
      <SharedParentPicker
        client={client}
        value={value}
        disabled={unavailable}
        organization={organization}
        onChange={onChange}
      />
      {value.kind === 'workspace_per_partition' ? (
        <ProfileSelection
          client={client}
          kind="workspace"
          value={value.workspace_profile}
          disabled={unavailable}
          organization={organization}
          onChange={(workspace_profile) => onChange({ ...value, workspace_profile })}
        />
      ) : null}
      <ProfileSelection
        client={client}
        kind="agent"
        value={value.agent_profile}
        disabled={unavailable}
        organization={organization}
        onChange={(agent_profile) => onChange({ ...value, agent_profile })}
      />
      <p>
        Profiles pin published revisions for future conversations. Existing resources keep their
        original profiles. The shared Sandbox’s model access, services and limits remain those of
        its provisioned profile.
      </p>
      {(['keep_other_workspaces', 'keep_other_agents_per_workspace'] as const).map((key) => (
        <label key={key}>
          {key === 'keep_other_workspaces'
            ? 'Workspace slots reserved for other work'
            : 'Agent slots per Workspace reserved for other work'}
          <Input
            type="number"
            min={1}
            max={15}
            step={1}
            value={value.capacity[key]}
            disabled={unavailable}
            onChange={(event) =>
              onChange({
                ...value,
                capacity: { ...value.capacity, [key]: Number(event.target.value) },
              })
            }
          />
        </label>
      ))}
      <p>
        Reserve 1 through 15 slots for other work. These reserves do not increase the parent’s
        limits.
      </p>
      <label className="bridges__check">
        <input
          type="checkbox"
          checked={editor.trustChecked}
          disabled={unavailable}
          onChange={(event) => editor.setTrust(event.target.checked)}
        />
        I acknowledge that all participants share filesystem, processes and credentials.
      </label>
      {editor.reason === undefined && value.sandbox_id !== '' ? (
        editor.access.state.status === 'ready' ? (
          <p role="status">
            Current management access and resource ancestry are verified. Saving and execution
            recheck authority.
          </p>
        ) : editor.access.state.status === 'error' ? (
          <p role="status">
            {editor.access.state.message}{' '}
            <Button onClick={editor.access.reload}>Check selected resource</Button>
          </p>
        ) : (
          <p role="status">
            Select the exact parent resources and verify current management access.
          </p>
        )
      ) : null}
    </div>
  );
}
