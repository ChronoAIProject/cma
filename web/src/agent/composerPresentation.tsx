import type { AgentStatus } from '../api/types';
import { Button } from '../ui/Button';
import type { SandboxInputAvailability } from './sandboxInput';
import { STOPPED_AGENT_MESSAGE } from './stopAgentCopy';

export function composerInputAvailability(
  availability: SandboxInputAvailability,
  readOnly: boolean,
  accessUncertain: boolean,
  connected: boolean,
): SandboxInputAvailability {
  if (readOnly)
    return {
      ...availability,
      allowed: false,
      notice: accessUncertain
        ? availability.notice
        : 'Only the owner can send messages to this agent.',
    };
  return connected ? availability : { ...availability, allowed: false };
}

export function canMessage(status: AgentStatus): boolean {
  return status === 'ready' || status === 'working' || status === 'paused';
}

export function placeholder(status: AgentStatus): string {
  if (status === 'starting') return 'Message Agent';
  if (status === 'waiting_approval') return 'Resolve the approval to continue.';
  if (status === 'stopped') return STOPPED_AGENT_MESSAGE;
  if (status === 'deleted') return 'This Agent was deleted.';
  if (status === 'failed') return 'This Agent is unavailable.';
  return 'Reply to Chrono Agent…';
}

export function FirstTextSendNotice({ onCancel }: { readonly onCancel: () => void }) {
  return (
    <div className="agent-composer__notice">
      <span role="status">Checking connections before sending…</span>{' '}
      <Button size="sm" onClick={onCancel}>
        Cancel send
      </Button>
    </div>
  );
}
