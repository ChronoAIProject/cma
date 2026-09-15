import type { BridgeWorkspaceCleanupHold, BridgeWorkspaceCleanupProgress } from '@chronoai/cma-sdk';
import type { AppRoute } from '../routing/routes';
import { ResourceLink } from '../resources/ResourceLink';
import { Button } from '../ui/Button';
import { RiskConfirmation } from '../ui/RiskConfirmation';
import type { WorkspaceCleanupController } from './useWorkspaceCleanup';

const progress: Record<BridgeWorkspaceCleanupProgress, string> = {
  not_requested: 'Workspace cleanup not requested',
  requested: 'Workspace cleanup requested',
  admission_closed: 'New Workspace work closed',
  removal_started: 'Workspace removal started',
  runtime_removed: 'Workspace files removed; confirmation pending',
  complete: 'Workspace cleanup complete',
};
const holds: Record<BridgeWorkspaceCleanupHold, string> = {
  allocation_not_retired: 'This allocation has not been retired.',
  workspace_not_owned:
    'This allocation did not create the Workspace. It cannot remove a reused Workspace.',
  child_cleanup_incomplete:
    'Complete the original child cleanup before requesting Workspace removal.',
  workspace_busy: 'The Workspace is busy.',
  other_workspace_children:
    'Additional Agent or Worktree history prevents whole Workspace removal.',
  inventory_limit: 'The resource inventory could not establish readiness.',
  authority_unavailable: 'Current original-owner cleanup authority is unavailable.',
  parent_unavailable: 'The parent Sandbox is unavailable for cleanup.',
  runtime_unsupported: 'This Sandbox does not currently support safe owned Workspace cleanup.',
  runtime_changed: 'The runtime used for removal could not be confirmed.',
  evidence_unavailable: 'Workspace removal evidence is currently unavailable.',
  evidence_inconsistent: 'Retained Workspace removal evidence is inconsistent.',
};
export function WorkspaceCleanup({
  controller: view,
  onNavigate,
}: {
  readonly controller: WorkspaceCleanupController;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const { status, command, review } = view;
  if (!view.supportedPlacement) return null;
  return (
    <section className="bridges__fields" aria-label="Owned Workspace cleanup">
      <h3>Owned Workspace cleanup</h3>
      <p>
        Removing the Workspace is a separate choice after child cleanup. Only the Workspace
        originally created by this allocation can be removed; the Sandbox, sibling Workspaces and
        retained history stay in place.
      </p>
      {!view.opened ? (
        <>
          {!view.childComplete ? (
            <p>Complete the original child cleanup before reviewing Workspace removal.</p>
          ) : null}
          <Button
            aria-label="Inspect Workspace cleanup"
            disabled={!view.childComplete || command.busy}
            onClick={view.inspect}
          >
            Inspect cleanup
          </Button>
        </>
      ) : (
        <>
          {view.observation.state.status === 'loading' ? (
            <p role="status">Checking owned Workspace cleanup and current access</p>
          ) : null}
          {view.observation.state.status === 'error' ? (
            <p role="alert">{view.observation.state.message}</p>
          ) : null}
          {status === undefined ? null : (
            <>
              <div className="bridges__allocation-target">
                <ResourceLink
                  route={{ kind: 'workspace', workspaceId: status.target.workspace_id }}
                  onNavigate={onNavigate}
                >
                  Workspace {status.target.workspace_id}
                </ResourceLink>
              </div>
              <h4>{progress[status.progress]}</h4>
              {status.receipt === null ? null : (
                <p>
                  Workspace removal request accepted{' '}
                  {new Date(status.receipt.accepted_at_ms).toLocaleString()}.
                </p>
              )}
              <p>
                {status.workspace_capacity_released
                  ? 'The owned Workspace reservation has been released.'
                  : 'The Workspace reservation has not been released. Acceptance alone does not confirm removal.'}
              </p>
              {status.holds.length === 0 ? null : (
                <ul aria-label="Workspace cleanup holds">
                  {status.holds.map((hold) => (
                    <li key={hold}>{holds[hold]}</li>
                  ))}
                </ul>
              )}
              {!status.runtime_supported && !status.holds.includes('runtime_unsupported') ? (
                <p>{holds.runtime_unsupported}</p>
              ) : null}
              {status.progress === 'not_requested' &&
              !status.eligible &&
              status.holds.length === 0 ? (
                <p>The current checks do not permit owned Workspace removal.</p>
              ) : null}
              {!view.childComplete ? (
                <p>
                  The completed original child receipt could not currently be confirmed. New
                  Workspace consent is unavailable.
                </p>
              ) : null}
              {view.childComplete && !view.childReceiptMatches ? (
                <p>
                  The original completed child receipt does not match this Workspace cleanup
                  observation. Refresh before reviewing removal.
                </p>
              ) : null}
              {status.observed_at_ms === null ? null : (
                <p>
                  Last Workspace cleanup observation:{' '}
                  {new Date(status.observed_at_ms).toLocaleString()}.
                </p>
              )}
              <p>The Sandbox, sibling Workspaces and retained history are preserved.</p>
              {view.acceptedWithoutObservation ? (
                <p role="status">
                  Workspace cleanup was accepted. Refresh its current progress before any further
                  action.
                </p>
              ) : null}
            </>
          )}
          {status?.holds.includes('workspace_not_owned') ? null : (
            <Button
              aria-label="Remove owned Workspace"
              variant="danger"
              disabled={!view.canRequest}
              onClick={view.reviewRemoval}
            >
              Remove Workspace
            </Button>
          )}
          <Button
            aria-label="Refresh Workspace cleanup"
            disabled={command.busy}
            onClick={view.observation.reload}
          >
            Refresh cleanup
          </Button>
          {command.busy ? (
            <p role="status">Checking acceptance of the original Workspace cleanup request</p>
          ) : null}
          {command.message === undefined ? null : <p role="status">{command.message}</p>}
          {command.pending === undefined ? null : (
            <Button
              aria-label="Recover original Workspace cleanup request"
              disabled={!view.canRecover}
              onClick={command.recover}
            >
              Recover request
            </Button>
          )}
        </>
      )}
      <RiskConfirmation
        open={review !== undefined}
        title="Remove this owned Workspace?"
        description={
          review === undefined
            ? ''
            : `Remove Workspace ${review.allocation.target.workspace_id} and its remaining files from generation ${review.allocation.generation}, after the original child cleanup has completed. The Sandbox, sibling Workspaces and retained history stay in place. This separate removal cannot be undone.`
        }
        acknowledgeLabel="I understand that this owned Workspace and its remaining files will be removed."
        confirmLabel="Request removal"
        busy={command.busy}
        confirmDisabled={!view.reviewCurrent}
        status={
          review !== undefined && !view.reviewCurrent
            ? 'Current access, revisions, child receipt or Workspace eligibility changed. Cancel and refresh before reviewing this Workspace again.'
            : undefined
        }
        onCancel={view.cancelReview}
        onConfirm={view.confirm}
      />
    </section>
  );
}
