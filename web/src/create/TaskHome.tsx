import { useSandboxCapacity } from '../sandbox/useSandboxCapacity';
import { SandboxCapacityPanel } from '../sandbox/SandboxCapacityPanel';
import { DocsLink } from '../services/DocsLink';
import { useEffect, useId, useRef, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import type { AppRoute } from '../routing/routes';
import { Button } from '../ui/Button';
import { taskSubmission, submitTaskOnShortcut } from './taskSubmission';
import { useTaskCreationSupport } from './useTaskCreationSupport';
import { TaskTemplateFields, useTaskTemplateSupport } from './TaskTemplateFields';
import type { TemplateIssue } from './taskTemplateModel';
import { useLifecycleIntent } from './lifecycleIntent';
import { lifecycleFailureMessage } from './lifecycleFailure';
import { useLifecycleAgent } from './lifecycleAgent';
import { useLifecycleTransport } from './lifecycleTransport';
import { AgentProvisioningProgress } from './AgentProvisioningProgress';
import { TaskComposer } from './TaskComposer';
import { RunTargetMenu } from './RunTargetMenu';
import { RepositoryChip } from './RepositoryChip';
import { RepositoryPreparationHelp } from './RepositoryPreparationHelp';
import { TaskTemplates } from './TaskTemplates';
import { AgentLifecycleCard } from './AgentLifecycleCard';
import { useTaskExperience } from '../taskExperience';
import { useTaskDraft } from './taskDraft';
import { useTaskTemplateDraftSwitch } from './TaskTemplateDraftSwitch';
import { useConnectionGate } from '../connections/useConnectionGate';
import '../ui/Input.css';
import './AgentLifecycleCard.css';
import './TaskHome.css';
import './TaskFeedback.css';

const recoverTask = { label: 'Recover original task', kind: 'recover' } as const;
const continueTask = { label: 'Continue original task', kind: 'recover' } as const;
const retryTaskStatus = { label: 'Retry task status', kind: 'status' } as const;

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
    <TaskHomeContent
      key={scope.generation}
      {...props}
      onRefresh={() => setScope((value) => ({ ...value, generation: value.generation + 1 }))}
    />
  );
}

function TaskHomeContent({
  client,
  viewerId,
  workspaceId,
  sandboxId,
  canCreateSandbox,
  onCreated,
  onRefresh,
}: TaskHomeProps & { readonly onRefresh: () => void }) {
  const policy = useTaskExperience(client, viewerId);
  const capacity = useSandboxCapacity(client, viewerId);
  const [checkingCapacity, setCheckingCapacity] = useState(false);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const [manual, setManual] = useState(false);
  const [recovering, setRecovering] = useState(false);
  const titleId = useId();
  const errorId = useId();
  const draft = useTaskDraft(viewerId, workspaceId, sandboxId);
  const input = useRef<HTMLTextAreaElement>(null);
  const [validation, setValidation] = useState<TemplateIssue>();
  const templateSupported = useTaskTemplateSupport(client);
  const form = useRef<HTMLFormElement>(null);
  const templateDraft = useTaskTemplateDraftSwitch(
    draft,
    (template) => {
      const previousFocus = document.activeElement;
      requestAnimationFrame(() => {
        if (document.activeElement !== previousFocus && document.activeElement !== document.body)
          return;
        (template
          ? form.current?.querySelector<HTMLInputElement>('[data-template-field]')
          : input.current
        )?.focus();
      });
    },
    () => setValidation(undefined),
  );
  const { supported, repositorySupported } = useTaskCreationSupport(client);
  const contextual = Boolean(workspaceId || sandboxId);
  const capacityUnavailable =
    !contextual && (!capacity.value || Boolean(capacity.error) || capacity.value.available === 0);
  const canCreate = Boolean(viewerId && (workspaceId || sandboxId || canCreateSandbox));
  const machineIncludedAvailable = Boolean(
    viewerId && !workspaceId && !sandboxId && canCreateSandbox,
  );
  const lifecycle = useLifecycleIntent(client, viewerId, workspaceId, sandboxId);
  const connections = useConnectionGate(Boolean(viewerId), client);
  const { intent, operation } = lifecycle;
  const quotaFailed =
    operation?.status === 'failed' && operation.failure?.code === 'sandbox_limit_reached';
  const failure = lifecycleFailureMessage(
    lifecycle.failure,
    intent?.receipt ? retryTaskStatus : lifecycle.connection ? continueTask : recoverTask,
  );
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
  const preparationStatus = operation?.status ?? intent?.receipt?.status;
  const preparing =
    (!recovering || Boolean(intent?.receipt)) &&
    !failure &&
    !lifecycle.connection &&
    preparationStatus !== 'failed' &&
    preparationStatus !== 'waiting_for_connection' &&
    preparationStatus !== 'succeeded' &&
    (lifecycle.pending || Boolean(intent?.receipt));
  const submission = taskSubmission(draft, {
    machine: machineIncludedAvailable,
    repository: repositorySupported,
    template: templateSupported,
  });
  const submissionDisabled =
    !policy.task_home_admission ||
    capacityUnavailable ||
    checkingCapacity ||
    lifecycle.pending ||
    lifecycle.blocked ||
    Boolean(intent) ||
    !canCreate ||
    supported !== true;
  return (
    <section
      className="task-home"
      data-task-home
      data-contextual={contextual || undefined}
      aria-labelledby={titleId}
    >
      {!contextual ? (
        <>
          <span className="task-home__label">
            <span className="task-home__dot" aria-hidden="true" />
            New agent
          </span>
          <h1 id={titleId}>What should your Agent work on?</h1>
          <p className="task-home__intro">
            Give it a task. Your Agent works through it on a cloud computer.{' '}
            <DocsLink view="start-a-task">Help starting a task</DocsLink>
          </p>
        </>
      ) : (
        <h2 className="sr-only" id={titleId}>
          New task in {workspaceId ? `Workspace ${workspaceId}` : `Sandbox ${sandboxId}`}
        </h2>
      )}
      {policy.settled && !policy.task_home_admission && !intent ? (
        <>
          <p className="task-home__status" role="status">
            Task creation is unavailable. You can create an Agent and start a conversation.
          </p>
          <Button
            variant="outline"
            placement="task-home__action"
            disabled={lifecycle.blocked || !canCreate}
            onClick={() => setManual(true)}
          >
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
        <p className="task-home__context">
          Creating in {workspaceId ? `Workspace ${workspaceId}` : `Sandbox ${sandboxId}`}
        </p>
      ) : null}
      {!contextual ? <SandboxCapacityPanel client={client} capacity={capacity} /> : null}
      {templateDraft.feedback}
      <form
        ref={form}
        noValidate
        onKeyDown={submitTaskOnShortcut}
        onSubmit={async (event) => {
          event.preventDefault();
          if (submissionDisabled) return;
          setValidation(submission.issue);
          if (submission.issue) {
            const target = submission.issue.field
              ? form.current?.querySelector<HTMLInputElement>(
                  `[data-template-field="${submission.issue.field}"]`,
                )
              : input.current;
            target?.focus();
            return;
          }
          const draftRevision = draft.persist();
          if (!draftRevision) return;
          if (!contextual) {
            setCheckingCapacity(true);
            const current = await capacity.ensureFresh();
            if (!mounted.current) return;
            setCheckingCapacity(false);
            if (!current || current.available === 0) return;
          }
          setRecovering(false);
          void lifecycle.submit(
            {
              name: 'New task',
              initial_message: submission.text,
              ...(draft.template ? { template: draft.template } : {}),
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
            },
            draftRevision,
          );
        }}
      >
        {draft.template ? (
          <TaskTemplateFields
            template={draft.template}
            issue={validation}
            errorId={errorId}
            onChange={(template) => {
              setValidation(undefined);
              draft.changeTemplate(template);
            }}
            onClear={templateDraft.clear}
          />
        ) : null}
        {draft.template && templateSupported !== true ? (
          <p className="task-home__status" role="status">
            Template tasks are not available on this service. You can keep editing or write a task
            without a template.
          </p>
        ) : null}
        <TaskComposer
          text={submission.text}
          template={Boolean(draft.template)}
          invalid={Boolean(draft.template && submission.issue)}
          input={input}
          validation={validation?.message}
          errorId={errorId}
          disabled={submissionDisabled}
          onChange={(text) => {
            setValidation(undefined);
            draft.change(text);
          }}
        >
          <RunTargetMenu
            workspaceId={workspaceId}
            sandboxId={sandboxId}
            recordedDesktop={intent?.body.desktop === true}
            machineIncluded={intent ? intent.body.machine_included === true : draft.machineIncluded}
            machineIncludedAvailable={machineIncludedAvailable}
            frozen={Boolean(intent)}
            onMachineChange={draft.changeMachineIncluded}
          />
          <span className="task-home__divider" aria-hidden="true" />
          <div
            className="task-home__repository-slot"
            data-has-repository={Boolean(draft.repositoryUrl || draft.repositoryRef) || undefined}
          >
            <RepositoryChip
              url={draft.repositoryUrl}
              reference={draft.repositoryRef}
              onChange={draft.changeRepository}
              checking={!contextual && repositorySupported === undefined}
              unavailableReason={
                workspaceId || sandboxId
                  ? 'Repository context is inherited. Start from Home to choose another repository.'
                  : repositorySupported === undefined
                    ? 'Checking repository support…'
                    : repositorySupported === false
                      ? 'Repository tasks are unavailable on this service.'
                      : undefined
              }
            />
          </div>
        </TaskComposer>
      </form>
      {preparing ? (
        <p className="task-home__preparing" role="status">
          <span className="task-home__dot" aria-hidden="true" />
          {contextual ? 'Preparing your Agent…' : 'Preparing a cloud computer for your Agent…'}
        </p>
      ) : null}
      <TaskTemplates onSelect={templateDraft.select} />
      <p className="task-home__defaults">
        <span className="task-home__dot" aria-hidden="true" />
        GPT-6 · High reasoning · Autonomous
      </p>
      {supported === undefined ? (
        <p className="sr-only" role="status">
          Checking task creation…
        </p>
      ) : null}
      {supported === false ? (
        <p className="task-home__status" role="status">
          Task creation is unavailable on this service.
        </p>
      ) : null}
      {!canCreate ? (
        <p className="task-home__alert" role="alert">
          You do not have permission to create a new Sandbox.
        </p>
      ) : null}
      {validation ? (
        <p className="task-home__alert" id={errorId} role="alert">
          {validation.message}
        </p>
      ) : null}
      {intent ? (
        <section className="task-submitted" aria-label="Submitted task">
          <h2>Submitted task</h2>
          <p className="task-submitted__text">{intent.body.initial_message}</p>
          {!intent.receipt && !lifecycle.pending ? (
            <DocsLink view="recover-and-continue">Help recovering this request</DocsLink>
          ) : null}
          {intent.body.repository ? (
            <>
              <p className="task-submitted__literal">{intent.body.repository.url}</p>
              <dl>
                <div>
                  <dt>Requested ref</dt>
                  <dd>{intent.body.repository.ref ?? 'Default branch'}</dd>
                </div>
                <div>
                  <dt>Checkout commit</dt>
                  <dd>{agent?.workspace.resolvedCommit ?? 'Not prepared yet'}</dd>
                </div>
              </dl>
            </>
          ) : null}
        </section>
      ) : null}
      {intent && !intent.receipt && !lifecycle.connection ? (
        <Button
          variant="outline"
          placement="task-home__action"
          disabled={lifecycle.pending || lifecycle.blocked}
          onClick={() => {
            setRecovering(true);
            void lifecycle.submit();
          }}
        >
          {recoverTask.label}
        </Button>
      ) : null}
      {connections.dialog}
      {lifecycle.connection ? (
        <Button
          variant="outline"
          placement="task-home__action"
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
          {continueTask.label}
        </Button>
      ) : null}
      {intent?.receipt && !preparing && !failure ? (
        <p className="task-home__status" role="status">
          {preparationStatus === 'failed'
            ? 'Task accepted, but Agent preparation failed.'
            : preparationStatus === 'waiting_for_connection'
              ? 'Task accepted. A connection check is required.'
              : preparationStatus === 'succeeded'
                ? 'Agent prepared. Verifying access to your task.'
                : 'Task accepted. Checking task preparation.'}
        </p>
      ) : null}
      {intent?.receipt && preparationStatus !== 'failed' ? (
        <p className="task-home__status">Task progress and results appear in the conversation.</p>
      ) : null}
      {intent?.receipt ? (
        <AgentProvisioningProgress
          receipt={intent.receipt}
          operation={operation}
          announce={!preparing}
          presentation="task"
        />
      ) : null}
      {quotaFailed ? (
        <p role="alert">
          The Sandbox environment limit is now reached. Your task draft is saved. Manage
          environments and check capacity before starting again.
        </p>
      ) : null}
      <RepositoryPreparationHelp
        client={client}
        viewerId={viewerId}
        intent={intent}
        operation={operation}
      />
      {intent?.receipt && operation?.status === 'failed' ? (
        <Button
          disabled={quotaFailed && capacityUnavailable}
          variant="outline"
          placement="task-home__action"
          onClick={() => {
            if (lifecycle.clear()) input.current?.focus();
          }}
        >
          Start another task
        </Button>
      ) : null}
      {draft.error ? (
        <p className="task-home__alert" role="alert">
          {draft.error}
        </p>
      ) : null}
      {failure ? (
        <p className="task-home__alert" role="alert">
          {failure}
        </p>
      ) : null}
      {failure && intent?.receipt ? (
        <Button variant="outline" placement="task-home__action" onClick={lifecycle.retryStatus}>
          {retryTaskStatus.label}
        </Button>
      ) : null}
      {connections.error ? (
        <p className="task-home__alert" role="alert">
          {connections.error}
        </p>
      ) : null}
    </section>
  );
}
