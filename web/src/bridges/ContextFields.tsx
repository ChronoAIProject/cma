import type { BridgeConfiguration, BridgePlacement } from '@chronoai/cma-sdk';
import type { BridgeViewsClient } from './client';
import { Picker } from '../ui/Picker';
import { ProfileSelection } from './ProfileSelection';
import { RetainedAgent } from './RetainedAgent';
import { SharedPlacementFields } from './SharedPlacementFields';
import { placementUnavailable, type PlacementEditor } from './usePlacementEditor';
import { initialConfiguration, initialSharedPlacement, partitions } from './configuration';

export function ContextFields({
  client,
  value,
  disabled,
  organization,
  editor,
  onChange,
}: {
  readonly client: BridgeViewsClient;
  readonly value: BridgeConfiguration;
  readonly disabled: boolean;
  readonly organization?: string;
  readonly editor: PlacementEditor;
  readonly onChange: (value: BridgeConfiguration) => void;
}) {
  const placement = value.placement;
  const changePlacement = (next: BridgePlacement) => onChange({ ...value, placement: next });
  return (
    <fieldset disabled={disabled} className="bridges__fields">
      <legend>Conversation and resources</legend>
      <label>
        Resource placement
        <Picker
          label="Resource placement"
          value={placement.kind}
          disabled={disabled}
          options={[
            { value: 'new_sandbox', label: 'A separate Sandbox for each conversation' },
            {
              value: 'existing_agent',
              label: 'One retained Agent',
              unavailable:
                organization === undefined
                  ? undefined
                  : 'Organization target selection needs an authorized full ancestry view',
            },
            {
              value: 'worktree_agent_per_partition',
              label: 'A new Agent in an existing Workspace',
              unavailable: placementUnavailable(
                editor.catalog,
                value.binding.platform,
                'worktree_agent_per_partition',
              ),
            },
            {
              value: 'workspace_per_partition',
              label: 'A new Workspace in an existing Sandbox',
              unavailable: placementUnavailable(
                editor.catalog,
                value.binding.platform,
                'workspace_per_partition',
              ),
            },
          ]}
          onChange={(kind) => {
            if (
              (kind === 'workspace_per_partition' || kind === 'worktree_agent_per_partition') &&
              placementUnavailable(editor.catalog, value.binding.platform, kind) === undefined
            ) {
              onChange({
                ...value,
                partition: 'conversation_sender',
                placement: initialSharedPlacement(kind),
              });
              return;
            }
            if (kind === 'existing_agent')
              onChange({
                ...value,
                partition: 'fixed_shared_agent',
                placement: {
                  kind,
                  target: { sandbox_id: '', workspace_id: '', worktree_id: '', agent_id: '' },
                },
              });
            else if (kind === 'new_sandbox')
              onChange({
                ...value,
                partition: 'conversation_sender',
                placement: initialConfiguration().placement,
              });
          }}
        />
      </label>
      <label>
        Who shares context
        <Picker
          label="Who shares context"
          disabled={disabled || placement.kind === 'existing_agent'}
          value={value.partition}
          options={partitions.map((item) => ({
            ...item,
            unavailable:
              item.value === 'fixed_shared_agent' && placement.kind !== 'existing_agent'
                ? 'Choose a retained Agent first'
                : undefined,
          }))}
          onChange={(partition) => onChange({ ...value, partition })}
        />
      </label>
      <p>
        {placement.kind === 'new_sandbox' || placement.kind === 'existing_agent'
          ? partitions.find((item) => item.value === value.partition)?.explanation
          : 'Each partition has its own Agent conversation inside the selected shared Sandbox.'}
      </p>
      {placement.kind === 'new_sandbox' ? (
        <>
          <p>
            Each conversation gets one Sandbox, one Workspace, one Worktree and one Agent using
            these published profile revisions.
          </p>
          <ProfileSelection
            client={client}
            kind="sandbox"
            value={placement.sandbox_profile}
            disabled={disabled}
            onChange={(sandbox_profile) => changePlacement({ ...placement, sandbox_profile })}
          />
          <ProfileSelection
            client={client}
            kind="workspace"
            value={placement.workspace_profile}
            disabled={disabled}
            onChange={(workspace_profile) => changePlacement({ ...placement, workspace_profile })}
          />
          <ProfileSelection
            client={client}
            kind="agent"
            value={placement.agent_profile}
            disabled={disabled}
            onChange={(agent_profile) => changePlacement({ ...placement, agent_profile })}
          />
          <p>
            Profile changes apply to future allocations. Existing conversations keep their original
            resources and profile revisions.
          </p>
        </>
      ) : placement.kind === 'existing_agent' ? (
        <RetainedAgent
          client={client}
          value={placement.target}
          disabled={disabled || organization !== undefined}
          onChange={(target) => changePlacement({ kind: 'existing_agent', target })}
        />
      ) : (
        <SharedPlacementFields
          client={client}
          value={placement}
          disabled={disabled}
          organization={organization}
          editor={editor}
          onChange={changePlacement}
        />
      )}
      <p>
        {organization === undefined
          ? 'You own the resulting resources. Their current CMA sharing permissions determine who else can access them.'
          : 'The selected organization owns the resulting resources. Its current roles and resource grants determine CMA access.'}
      </p>
      <p>
        Replies go through the original bot to the original chat or thread. In a group, everyone who
        can read that chat can see the bot’s reply.
      </p>
    </fieldset>
  );
}
