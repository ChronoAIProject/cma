import type { AgentLifecycleCreateReceipt, AgentLifecycleOperationView } from '../api/client';

export function AgentProvisioningProgress({
  receipt,
  operation,
}: {
  readonly receipt: AgentLifecycleCreateReceipt;
  readonly operation?: AgentLifecycleOperationView;
}) {
  const status = operation?.status ?? receipt.status ?? 'queued';
  const readyPhases = new Set(
    operation?.events.filter((event) => event.status === 'ready').map((event) => event.phase),
  );
  const sandboxReady = readyPhases.has('sandbox_ready');
  const runtimeReady = readyPhases.has('workspace_runtime');
  const keyScopeReady = readyPhases.has('key_scope_ready') || status === 'succeeded';
  const state = (done: boolean, previous: boolean) =>
    done ? 'Ready' : previous ? 'In progress' : 'Waiting';
  return (
    <section className="agent-lifecycle-provisioning" aria-label="Agent provisioning progress">
      <h3>{status === 'failed' ? 'Agent provisioning needs attention' : 'Agent accepted'}</h3>
      <p>
        {status === 'failed'
          ? (operation?.error ?? 'Agent provisioning failed.')
          : status === 'waiting_for_connection'
            ? 'Connect the required services to continue setting up your Agent.'
            : status === 'succeeded'
              ? 'Your Agent is ready to open.'
              : 'Your Agent is being prepared. You can close this dialog and return to follow its progress.'}
      </p>
      <ol>
        <li data-state={state(sandboxReady, true)}>
          <strong>Sandbox ready</strong>
          <span>{state(sandboxReady, true)}</span>
        </li>
        <li data-state={state(runtimeReady, sandboxReady)}>
          <strong>
            Workspace + skills + model + reasoning effort + system prompt + permission level setup
            ready
          </strong>
          <span>{state(runtimeReady, sandboxReady)}</span>
        </li>
        <li data-state={state(keyScopeReady, runtimeReady)}>
          <strong>Agent key scope configuration ready</strong>
          <span>{state(keyScopeReady, runtimeReady)}</span>
        </li>
      </ol>
    </section>
  );
}
