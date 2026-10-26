import type { SandboxSummary } from '../api/types';
import type { TreePermissions } from '../shell/Tree';
import type { MenuEntry } from '../ui/Menu';
import {
  IconPlusOutline16,
  IconSettingsOutline16,
  IconShareOutline16,
  IconTrashOutline16,
} from '../ui/icons';
import { sandboxChildrenAvailable } from './lifecycle';

/** What the page knows besides the Sandbox record when it offers an action. */
export interface SandboxActionContext {
  /** Another action on this Sandbox is still being submitted or confirmed. */
  readonly busy: boolean;
  /** The last read failed or is being revalidated, so eligibility is not current. */
  readonly stale: boolean;
  /** The owner's service connections have not been confirmed yet. */
  readonly needsConnections: boolean;
}

const CHECKING = 'Checking the current Sandbox status.';
const WAIT = 'Wait for the current Sandbox operation to finish.';

/**
 * Why a new Workspace or Agent cannot start in this Sandbox right now.
 *
 * Children are admitted only into an Active Sandbox with no recorded
 * operation (CLAUDE.md §5: never accept work an outgoing runtime could lose),
 * so an unavailable create names which of those conditions is missing.
 */
export function sandboxCreateReason(
  sandbox: SandboxSummary,
  context: SandboxActionContext,
): string | undefined {
  if (context.stale) return CHECKING;
  if (sandbox.lifecycle?.operation !== undefined) return WAIT;
  if (!sandboxChildrenAvailable(sandbox))
    return 'Agents and Workspaces can start when the Sandbox is active.';
  if (context.busy) return WAIT;
  if (context.needsConnections) return 'Checking your service connections.';
  return undefined;
}

function deleteReason(sandbox: SandboxSummary, context: SandboxActionContext) {
  if (sandbox.status === 'retired') return 'This Sandbox has been deleted.';
  if (sandbox.status === 'retiring') return 'Deletion is already in progress.';
  if (context.stale) return CHECKING;
  if (
    context.busy ||
    sandbox.lifecycle?.checkpoint !== undefined ||
    sandbox.lifecycle?.operation !== undefined
  )
    return WAIT;
  return undefined;
}

export type SandboxMenuAction =
  'new_agent' | 'new_workspace' | 'sharing' | 'technical_details' | 'delete_sandbox';

/**
 * The Sandbox header's More actions — `docs/Web_Style.md` §17.
 *
 * Routine rows come first and Delete Sandbox sits last behind a separator, so
 * a slip down the list lands on something reversible. Sharing is listed only
 * once current access allows managing it; every other row stays in place
 * when unavailable and says why.
 */
export function sandboxHeaderMenu(
  sandbox: SandboxSummary,
  permissions: TreePermissions,
  context: SandboxActionContext,
  sharingAllowed: boolean,
): MenuEntry[] {
  const entries: MenuEntry[] = [];
  if (permissions.createAgent) {
    const reason = sandboxCreateReason(sandbox, context);
    entries.push({
      id: 'new_agent',
      label: 'New agent',
      icon: <IconPlusOutline16 />,
      disabled: reason !== undefined,
      reason,
    });
  }
  if (permissions.createWorkspace) {
    const reason = sandboxCreateReason(sandbox, context);
    entries.push({
      id: 'new_workspace',
      label: 'New workspace',
      icon: <IconPlusOutline16 />,
      disabled: reason !== undefined,
      reason,
    });
  }
  if (sharingAllowed)
    entries.push({ id: 'sharing', label: 'Sharing', icon: <IconShareOutline16 /> });
  entries.push({
    id: 'technical_details',
    label: 'Technical details',
    icon: <IconSettingsOutline16 />,
  });
  if (permissions.deleteSandbox) {
    const reason = deleteReason(sandbox, context);
    entries.push(
      { type: 'separator', id: 'before-delete' },
      {
        id: 'delete_sandbox',
        label: 'Delete Sandbox',
        icon: <IconTrashOutline16 />,
        danger: true,
        disabled: reason !== undefined,
        reason,
      },
    );
  }
  return entries;
}
