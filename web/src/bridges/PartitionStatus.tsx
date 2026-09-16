import type {
  BridgePartition,
  BridgePartitionActivity,
  BridgePartitionCommand,
} from '@chronoai/cma-sdk';
import type { AppRoute } from '../routing/routes';
import { ResourceLink } from '../resources/ResourceLink';

export function PartitionSummary({
  partition,
  onNavigate,
}: {
  readonly partition: BridgePartition;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const source = partition.source;
  const target = partition.target;
  return (
    <div>
      <strong>
        Generation {partition.generation} ·{' '}
        {partition.current ? 'Current partition' : 'Previous partition'}
      </strong>
      <p>
        {partition.state.replaceAll('_', ' ')} · {partition.placement.replaceAll('_', ' ')}
      </p>
      {source.status === 'available' ? (
        <>
          <p>
            {source.sender_display_name ?? source.sender_id} · {source.platform} chat{' '}
            {source.chat_id}
          </p>
          <p>
            First accepted sender: {source.sender_id}. Context sharing:{' '}
            {partition.context_sharing.replaceAll('_', ' ')}.
          </p>
        </>
      ) : (
        <p>
          {source.status === 'redacted'
            ? 'Source and target details are restricted by current resource access.'
            : 'The first accepted source is no longer available.'}
        </p>
      )}
      {target.status === 'redacted' ? null : (
        <div className="bridges__allocation-target">
          {target.status === 'available' && target.present.includes('sandbox') ? (
            <ResourceLink
              route={{ kind: 'sandbox', sandboxId: target.ids.sandbox_id }}
              onNavigate={onNavigate}
            >
              Sandbox {target.ids.sandbox_id}
            </ResourceLink>
          ) : (
            <span>Reserved Sandbox {target.ids.sandbox_id}</span>
          )}
          {target.status === 'available' && target.present.includes('workspace') ? (
            <ResourceLink
              route={{ kind: 'workspace', workspaceId: target.ids.workspace_id }}
              onNavigate={onNavigate}
            >
              Workspace {target.ids.workspace_id}
            </ResourceLink>
          ) : (
            <span>Reserved Workspace {target.ids.workspace_id}</span>
          )}
          <span>Worktree {target.ids.worktree_id}</span>
          {target.status === 'available' && target.present.includes('agent') ? (
            <ResourceLink
              route={{ kind: 'agent', agentId: target.ids.agent_id }}
              onNavigate={onNavigate}
            >
              Open Agent conversation {target.ids.agent_id}
            </ResourceLink>
          ) : (
            <span>Reserved Agent {target.ids.agent_id}</span>
          )}
        </div>
      )}
      <p>
        Allocation {partition.id} · Revision {partition.revision} · Configuration{' '}
        {partition.config_revision}
      </p>
    </div>
  );
}

function CommandStatus({ command }: { readonly command: BridgePartitionCommand }) {
  const wait = {
    none: '',
    worker: 'Waiting for a worker',
    approval: 'Waiting for approval',
    action: 'Waiting for action',
  }[command.waiting_for];
  return (
    <div>
      <strong>{wait || command.status.replaceAll('_', ' ')}</strong>
      <p>
        Command {command.command_id} · Response {command.response_id}
      </p>
      <p>
        Agent order {command.agent_order}
        {command.turn_sequence === null ? '' : ` · Turn ${command.turn_sequence}`}
        {command.delivery === null ? '' : ` · ${command.delivery.replaceAll('_', ' ')}`}
      </p>
    </div>
  );
}

export function PartitionActivity({ activity }: { readonly activity: BridgePartitionActivity }) {
  if (activity.status === 'redacted')
    return <p>Conversation activity is restricted by current resource access.</p>;
  if (activity.status === 'unavailable') return <p>The target Agent is not available yet.</p>;
  return (
    <div>
      <h4>Channel activity</h4>
      <p>
        Recorded Agent state: {activity.agent_status.replaceAll('_', ' ')}. Check the Agent
        conversation for current connection status.
      </p>
      {activity.source_unavailable ? (
        <p role="status">
          Some accepted message sources are no longer available. This activity view is incomplete.
        </p>
      ) : null}
      {activity.pending_truncated ? (
        <p role="status">
          More than 64 commands are pending. This view shows the first available commands from that
          bounded set.
        </p>
      ) : null}
      {activity.pending.length === 0 ? (
        <p>
          {activity.source_unavailable
            ? 'No pending commands with retained source evidence.'
            : 'No pending Channel commands.'}
        </p>
      ) : (
        <ul aria-label="Pending Channel commands">
          {activity.pending.map((command) => (
            <li key={command.command_id}>
              <CommandStatus command={command} />
            </li>
          ))}
        </ul>
      )}
      <h4>Latest Channel response</h4>
      {activity.latest === null ? (
        <p>No response with retained source evidence is available.</p>
      ) : (
        <CommandStatus command={activity.latest} />
      )}
    </div>
  );
}
