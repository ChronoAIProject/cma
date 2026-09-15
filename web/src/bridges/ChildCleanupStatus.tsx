import type {
  BridgeChildCleanupHold,
  BridgeChildCleanupProgress,
  BridgeChildCleanupStatus,
} from '@chronoai/cma-sdk';

const progress: Record<BridgeChildCleanupProgress, string> = {
  not_requested: 'Cleanup not requested',
  requested: 'Cleanup requested',
  admission_closed: 'New child input closed',
  removal_started: 'Child removal started',
  process_closed: 'Agent process closed',
  home_removed: 'Agent home removed',
  worktree_removed: 'Worktree removed',
  complete: 'Child cleanup complete',
};
const holds: Record<BridgeChildCleanupHold, string> = {
  agent_busy: 'The Agent is busy.',
  workspace_busy: 'The Workspace is busy.',
  pending_command: 'An accepted command is still pending.',
  uncertain_command: 'An earlier command outcome is unknown.',
  pending_reply: 'An accepted channel reply is still pending.',
  uncertain_reply: 'An earlier channel reply outcome is unknown.',
  other_workspace_children: 'Other children remain in the Workspace.',
  inventory_limit: 'The bounded resource inventory could not establish readiness.',
  authority_unavailable: 'Current cleanup authority is unavailable.',
  parent_unavailable: 'The parent is unavailable for cleanup.',
  runtime_changed: 'The original runtime could not be confirmed.',
  evidence_unavailable: 'Removal evidence is currently unavailable.',
  evidence_inconsistent: 'Retained removal evidence is inconsistent.',
};

export function ChildCleanupStatus({ status }: { readonly status: BridgeChildCleanupStatus }) {
  return (
    <div aria-live="polite">
      <h3>{progress[status.progress]}</h3>
      {status.receipt === null ? null : (
        <p>Request accepted {new Date(status.receipt.accepted_at_ms).toLocaleString()}.</p>
      )}
      <p>
        {status.agent_capacity_released
          ? 'The Agent reservation has been released.'
          : 'The Agent reservation has not been released. Acceptance alone does not confirm removal.'}
      </p>
      {status.holds.length === 0 ? null : (
        <ul aria-label="Cleanup holds">
          {status.holds.map((hold) => (
            <li key={hold}>{holds[hold]}</li>
          ))}
        </ul>
      )}
      {status.observed_at_ms === null ? null : (
        <p>
          Last cleanup observation: {new Date(status.observed_at_ms).toLocaleString()}. This
          observation may predate acceptance.
        </p>
      )}
      <p>
        The Workspace, sibling Agents and retained history are preserved. Its Workspace reservation
        remains in place.
      </p>
    </div>
  );
}
