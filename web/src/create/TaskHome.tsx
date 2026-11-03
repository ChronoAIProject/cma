import { useEffect, useId, useRef, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import type { AppRoute } from '../routing/routes';
import { Button } from '../ui/Button';
import { useLifecycleIntent } from './lifecycleIntent';
import { useLifecycleAgent } from './lifecycleAgent';
import { useLifecycleTransport } from './lifecycleTransport';
import { AgentProvisioningProgress } from './AgentProvisioningProgress';
import { useTaskDraft } from './taskDraft';
import { useConnectionGate } from '../connections/useConnectionGate';
import '../ui/Input.css';
import './AgentLifecycleCard.css';
import './TaskHome.css';

const templates = [
  ['Research', 'Compare three tools and summarize their strengths, limitations, and sources.'],
  ['Spreadsheet', 'Create a spreadsheet to compare options, with clear columns and formulas.'],
  ['Document', 'Write a concise document explaining the problem, findings, and next steps.'],
  ['Website', 'Build a responsive website for this idea: '],
  ['Code', 'Implement this feature and verify its behavior: '],
] as const;

export interface TaskHomeProps {
  readonly client: CmaWebClient;
  readonly viewerId?: string;
  readonly workspaceId?: string;
  readonly sandboxId?: string;
  readonly canCreateSandbox: boolean;
  readonly onCreated: (route: AppRoute) => void;
}

export function TaskHome(props: TaskHomeProps) {
  const { client, viewerId, workspaceId, sandboxId } = props;
  const [scope, setScope] = useState({ client, viewerId, workspaceId, sandboxId, generation: 0 });
  if (
    scope.client !== client ||
    scope.viewerId !== viewerId ||
    scope.workspaceId !== workspaceId ||
    scope.sandboxId !== sandboxId
  )
    setScope({ client, viewerId, workspaceId, sandboxId, generation: scope.generation + 1 });
  return <TaskComposer key={scope.generation} {...props} />;
}

function TaskComposer({
  client,
  viewerId,
  workspaceId,
  sandboxId,
  canCreateSandbox,
  onCreated,
}: TaskHomeProps) {
  const inputId = useId();
  const titleId = useId();
  const errorId = useId();
  const draft = useTaskDraft(viewerId, workspaceId, sandboxId);
  const input = useRef<HTMLTextAreaElement>(null);
  const [validation, setValidation] = useState<string>();
  const [supported, setSupported] = useState<boolean | undefined>(() =>
    client.readTaskCreationSupport ? undefined : false,
  );
  useEffect(() => {
    const abort = new AbortController();
    if (!client.readTaskCreationSupport) return;
    void client.readTaskCreationSupport({ signal: abort.signal }).then(
      (value) => {
        if (!abort.signal.aborted) setSupported(value);
      },
      () => {
        if (!abort.signal.aborted) setSupported(false);
      },
    );
    return () => abort.abort();
  }, [client]);
  const canCreate = Boolean(viewerId && (workspaceId || sandboxId || canCreateSandbox));
  const lifecycle = useLifecycleIntent(client, viewerId, workspaceId, sandboxId);
  const connections = useConnectionGate(Boolean(viewerId), client);
  const { intent, operation } = lifecycle;
  const agent = useLifecycleAgent(
    client,
    operation,
    intent?.receipt?.id,
    viewerId,
    workspaceId,
    sandboxId,
  );
  const handoff = useLifecycleTransport(client, operation, intent?.receipt?.id, viewerId);
  const opened = useRef<string | undefined>(undefined);
  useEffect(() => {
    if (!agent || !operation || opened.current === operation.id) return;
    if (!lifecycle.bindAgent(agent.id)) return;
    opened.current = operation.id;
    handoff();
    onCreated({ kind: 'agent', agentId: agent.id });
  }, [agent, handoff, lifecycle, onCreated, operation]);
  return (
    <section className="task-home" aria-labelledby={titleId}>
      <h1 id={titleId}>What would you like to do?</h1>
      {workspaceId || sandboxId ? (
        <p>Creating in {workspaceId ? `Workspace ${workspaceId}` : `Sandbox ${sandboxId}`}</p>
      ) : null}
      <form
        onSubmit={(event) => {
          event.preventDefault();
          if (!canCreate || supported !== true || intent || lifecycle.pending) return;
          const invalid = !draft.text.trim()
            ? 'Describe a task before starting.'
            : new TextEncoder().encode(draft.text).length > 262144
              ? 'Your task exceeds the 262144-byte limit.'
              : undefined;
          setValidation(invalid);
          if (invalid) return;
          if (!draft.persist()) return;
          void lifecycle.submit({
            name: 'New task',
            initial_message: draft.text,
            ...(workspaceId ? { workspace_id: workspaceId } : {}),
            ...(sandboxId ? { sandbox_id: sandboxId } : {}),
          });
        }}
      >
        <label className="field__label" htmlFor={inputId}>
          Your task
        </label>
        <div className="input task-home__field">
          <textarea
            className="input__control"
            ref={input}
            id={inputId}
            value={draft.text}
            aria-invalid={validation !== undefined}
            aria-describedby={validation ? errorId : undefined}
            onChange={(event) => {
              setValidation(undefined);
              draft.change(event.target.value);
            }}
          />
        </div>
        <Button
          type="submit"
          variant="primary"
          disabled={lifecycle.pending || Boolean(intent) || !canCreate || supported !== true}
        >
          Start task
        </Button>
      </form>
      <div className="task-home__templates" role="group" aria-label="Task templates">
        {templates.map(([name, text]) => (
          <Button
            key={name}
            onClick={() => {
              draft.change(text);
              input.current?.focus();
            }}
          >
            {name}
          </Button>
        ))}
      </div>
      {supported === undefined ? <p role="status">Checking task creation…</p> : null}
      {supported === false ? (
        <p role="status">Task creation is unavailable on this service.</p>
      ) : null}
      {!canCreate ? <p role="alert">You do not have permission to create a new Sandbox.</p> : null}
      {validation ? (
        <p id={errorId} role="alert">
          {validation}
        </p>
      ) : null}
      {intent ? (
        <section aria-label="Retained task">
          <h2>Submitted task</h2>
          <pre>{intent.body.initial_message}</pre>
        </section>
      ) : null}
      {intent && !intent.receipt && !lifecycle.connection ? (
        <Button
          disabled={lifecycle.pending || lifecycle.blocked}
          onClick={() => void lifecycle.submit()}
        >
          Recover original task
        </Button>
      ) : null}
      {connections.dialog}
      {lifecycle.connection ? (
        <Button
          disabled={lifecycle.pending || lifecycle.blocked || connections.pending}
          onClick={() => {
            if (viewerId && lifecycle.connection)
              void connections.checkAndRun(
                lifecycle.connection.target,
                viewerId,
                () => void lifecycle.submit(),
              );
          }}
        >
          Continue original task
        </Button>
      ) : null}
      {intent?.receipt ? (
        <p role="status">
          {operation?.status === 'failed'
            ? 'Task accepted, but Agent preparation failed.'
            : operation?.status === 'waiting_for_connection'
              ? 'Task accepted. A connection check is required.'
              : operation?.status === 'succeeded'
                ? 'Agent prepared. Verifying access to your task.'
                : 'Task accepted. Preparing your Agent.'}
        </p>
      ) : null}
      {intent?.receipt ? <p>Task progress and results appear in the conversation.</p> : null}
      {intent?.receipt ? (
        <AgentProvisioningProgress receipt={intent.receipt} operation={operation} />
      ) : null}
      {intent?.receipt && operation?.status === 'failed' ? (
        <Button
          onClick={() => {
            if (lifecycle.clear()) input.current?.focus();
          }}
        >
          Start another task
        </Button>
      ) : null}
      {draft.error ? <p role="alert">{draft.error}</p> : null}
      {lifecycle.failure ? <p role="alert">{lifecycle.failure}</p> : null}
      {lifecycle.failure && intent?.receipt ? (
        <Button onClick={lifecycle.retryStatus}>Retry task status</Button>
      ) : null}
      {connections.error ? <p role="alert">{connections.error}</p> : null}
    </section>
  );
}
