import { useEffect, useId, useRef, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import type { AppRoute } from '../routing/routes';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { IconSendOutline16 } from '../ui/icons';
import { validRepository } from './repository';
import { useLifecycleIntent } from './lifecycleIntent';
import { useLifecycleAgent } from './lifecycleAgent';
import { useLifecycleTransport } from './lifecycleTransport';
import { AgentProvisioningProgress } from './AgentProvisioningProgress';
import { DesktopOption } from './DesktopOption';
import { MachineOption } from './MachineOption';
import { AgentLifecycleCard } from './AgentLifecycleCard';
import { useTaskExperience } from '../taskExperience';
import { useTaskDraft } from './taskDraft';
import { useConnectionGate } from '../connections/useConnectionGate';
import '../ui/Input.css';
import './AgentLifecycleCard.css';
import './TaskHome.css';

const templates = [
  [
    'Research a topic',
    'Compare three tools and summarize their strengths, limitations, and sources.',
  ],
  [
    'Analyze a spreadsheet',
    'Create a spreadsheet to compare options, with clear columns and formulas.',
  ],
  [
    'Write a document',
    'Write a concise document explaining the problem, findings, and next steps.',
  ],
  ['Build a website', 'Build a responsive website for this idea: '],
  ['Write code', 'Implement this feature and verify its behavior: '],
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
  return (
    <TaskComposer
      key={scope.generation}
      {...props}
      onRefresh={() => setScope((value) => ({ ...value, generation: value.generation + 1 }))}
    />
  );
}

function TaskComposer({
  client,
  viewerId,
  workspaceId,
  sandboxId,
  canCreateSandbox,
  onCreated,
  onRefresh,
}: TaskHomeProps & { readonly onRefresh: () => void }) {
  const policy = useTaskExperience(client, viewerId);
  const [manual, setManual] = useState(false);
  const inputId = useId();
  const titleId = useId();
  const errorId = useId();
  const draft = useTaskDraft(viewerId, workspaceId, sandboxId);
  const input = useRef<HTMLTextAreaElement>(null);
  const [validation, setValidation] = useState<string>();
  const [desktopAvailable, setDesktopAvailable] = useState(false);
  const [repositorySupported, setRepositorySupported] = useState(false);
  useEffect(() => {
    const abort = new AbortController();
    void client.readTaskRepositorySupport?.({ signal: abort.signal }).then(
      (value) => {
        if (!abort.signal.aborted) setRepositorySupported(value);
      },
      () => {
        if (!abort.signal.aborted) setRepositorySupported(false);
      },
    );
    return () => abort.abort();
  }, [client]);
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
  const machineIncludedAvailable = Boolean(
    viewerId && !workspaceId && !sandboxId && canCreateSandbox,
  );
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
  const handoff = useLifecycleTransport(
    client,
    operation,
    intent?.receipt?.id,
    viewerId,
    intent?.body.machine_included === true,
  );
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
      <h1 id={titleId}>What should your Agent work on?</h1>
      <p className="task-home__intro">
        It gets its own computer, works through the task step by step, and asks you when it needs a
        decision.
      </p>
      {!policy.task_home_admission && !intent ? (
        <>
          <p role="status">
            Task creation is unavailable. You can create an Agent and start a conversation.
          </p>
          <Button disabled={lifecycle.blocked || !canCreate} onClick={() => setManual(true)}>
            New agent
          </Button>
        </>
      ) : null}
      {manual ? (
        <AgentLifecycleCard
          client={client}
          viewerId={viewerId}
          workspaceId={workspaceId}
          sandboxId={sandboxId}
          canCreateSandbox={canCreateSandbox}
          onClose={onRefresh}
          onCreated={(receipt) => onCreated({ kind: 'agent', agentId: receipt.id })}
        />
      ) : null}
      {workspaceId || sandboxId ? (
        <p>Creating in {workspaceId ? `Workspace ${workspaceId}` : `Sandbox ${sandboxId}`}</p>
      ) : null}
      <form
        onSubmit={(event) => {
          event.preventDefault();
          if (
            !policy.task_home_admission ||
            !canCreate ||
            supported !== true ||
            intent ||
            lifecycle.pending
          )
            return;
          const invalid =
            draft.machineIncluded && !machineIncludedAvailable
              ? 'A shared desktop requires a new Sandbox. Keep the task and clear the shared desktop option.'
              : draft.desktop && (!desktopAvailable || !policy.desktop_enable)
                ? 'Desktop is unavailable. Keep the task and clear the desktop option, or try again later.'
                : !draft.text.trim()
                  ? 'Describe a task before starting.'
                  : new TextEncoder().encode(draft.text).length > 262144
                    ? 'Your task exceeds the 262144-byte limit.'
                    : draft.repositoryUrl && !repositorySupported
                      ? 'Repository tasks are unavailable on this service. Clear the repository to start an empty workspace.'
                      : !validRepository(draft.repositoryUrl)
                        ? 'Enter an HTTPS GitHub repository URL.'
                        : draft.repositoryRef &&
                            (!draft.repositoryUrl || !validRepositoryRef(draft.repositoryRef))
                          ? 'Enter a valid repository ref with a repository URL.'
                          : undefined;
          setValidation(invalid);
          if (invalid) return;
          if (!draft.persist()) return;
          void lifecycle.submit({
            name: 'New task',
            initial_message: draft.text,
            ...(draft.desktop ? { desktop: true } : {}),
            ...(draft.machineIncluded ? { machine_included: true } : {}),
            ...(!workspaceId && !sandboxId && draft.repositoryUrl
              ? {
                  repository: {
                    url: draft.repositoryUrl,
                    ...(draft.repositoryRef ? { ref: draft.repositoryRef } : {}),
                  },
                }
              : {}),
            ...(workspaceId ? { workspace_id: workspaceId } : {}),
            ...(sandboxId ? { sandbox_id: sandboxId } : {}),
          });
        }}
      >
        <div className="task-home__composer">
          <textarea
            className="task-home__input"
            aria-label="Your task"
            placeholder="Describe a task, ask a question, or share an idea…"
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
          <div className="task-home__options">
            <details className="task-home__option">
              <summary>{draft.repositoryUrl ? 'Repository added' : 'Add a GitHub repo'}</summary>
              <div className="task-home__settings">
                {!workspaceId &&
                !sandboxId &&
                (repositorySupported || draft.repositoryUrl || draft.repositoryRef) ? (
                  <>
                    <label className="field__label" htmlFor={`${inputId}-repository`}>
                      GitHub repository
                    </label>
                    <Input
                      id={`${inputId}-repository`}
                      type="url"
                      value={draft.repositoryUrl}
                      placeholder="Optional · https://github.com/owner/repository"
                      onChange={(event) =>
                        draft.changeRepository(event.target.value, draft.repositoryRef)
                      }
                    />
                    <label className="field__label" htmlFor={`${inputId}-ref`}>
                      Repository ref
                    </label>
                    <Input
                      id={`${inputId}-ref`}
                      value={draft.repositoryRef}
                      placeholder="Default branch"
                      onChange={(event) =>
                        draft.changeRepository(draft.repositoryUrl, event.target.value)
                      }
                    />
                    <p>The requested ref is verified during preparation.</p>
                    {draft.repositoryUrl || draft.repositoryRef ? (
                      <Button onClick={() => draft.changeRepository('', '')}>
                        Clear repository
                      </Button>
                    ) : null}
                  </>
                ) : (
                  <p>
                    {workspaceId || sandboxId
                      ? 'Repository context is inherited. Start from Home to choose another repository.'
                      : 'Repository tasks are unavailable on this service.'}
                  </p>
                )}
              </div>
            </details>
            <details className="task-home__option">
              <summary>
                {draft.machineIncluded || draft.desktop ? 'Desktop selected' : 'Desktop options'}
              </summary>
              <div className="task-home__settings">
                <MachineOption
                  available={machineIncludedAvailable}
                  enabled={intent ? intent.body.machine_included === true : draft.machineIncluded}
                  frozen={Boolean(intent)}
                  onChange={draft.changeMachineIncluded}
                />
                <DesktopOption
                  policyAllowed={policy.desktop_enable}
                  client={client}
                  workspaceId={workspaceId}
                  sandboxId={sandboxId}
                  enabled={intent ? intent.body.desktop === true : draft.desktop}
                  frozen={Boolean(intent)}
                  onChange={draft.changeDesktop}
                  onAvailable={setDesktopAvailable}
                />
              </div>
            </details>
            <Button
              type="submit"
              variant="primary"
              icon={<IconSendOutline16 />}
              aria-label="Start task"
              disabled={
                !policy.task_home_admission ||
                lifecycle.pending ||
                Boolean(intent) ||
                !canCreate ||
                supported !== true
              }
            />
          </div>
        </div>
      </form>
      <div className="task-home__templates" role="group" aria-label="Task templates">
        {templates.map(([name, text]) => (
          <Button
            key={name}
            variant="outline"
            onClick={() => {
              draft.change(text);
              input.current?.focus();
            }}
          >
            {name}
          </Button>
        ))}
      </div>
      <p className="task-home__defaults">GPT-6 · High reasoning · Autonomous</p>
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
        <section aria-label="Submitted task">
          <h2>Submitted task</h2>
          <pre>{intent.body.initial_message}</pre>
          {intent.body.repository ? (
            <>
              <pre>{intent.body.repository.url}</pre>
              <p>Requested ref: {intent.body.repository.ref ?? 'Default branch'}</p>
              <p>Checkout commit: {agent?.workspace.resolvedCommit ?? 'Not prepared yet'}</p>
            </>
          ) : null}
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

function validRepositoryRef(value: string) {
  return (
    new TextEncoder().encode(value).length <= 255 &&
    !value.startsWith('-') &&
    ![...value].some(
      (character) =>
        character.charCodeAt(0) < 33 ||
        character.charCodeAt(0) === 127 ||
        /\s/.test(character) ||
        '~^:?*[\\'.includes(character),
    ) &&
    !value.includes('..') &&
    !value.includes('@{')
  );
}
