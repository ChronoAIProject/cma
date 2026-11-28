import { useSandboxCapacity } from '../sandbox/useSandboxCapacity';
import { SandboxCapacityPanel } from '../sandbox/SandboxCapacityPanel';
import { useEffect, useId, useRef, useState } from 'react';
import type { TaskHomeProps } from './TaskHome.types';
export type { TaskHomeProps } from './TaskHome.types';
import { Button } from '../ui/Button';
import { taskSubmission, submitTaskOnShortcut, taskTemplateForSubmission } from './taskSubmission';
import { useTaskCreationSupport } from './useTaskCreationSupport';
import { useTaskTemplateAvailability } from './TaskTemplateFields';
import type { TemplateIssue } from './taskTemplateModel';
import { useLifecycleIntent } from './lifecycleIntent';
import { lifecycleFailureMessage } from './lifecycleFailure';
import { useLifecycleAgent } from './lifecycleAgent';
import { useLifecycleTransport } from './lifecycleTransport';
import { TaskCreationStatus, TaskQuotaNotice } from './TaskCreationStatus';
import { TaskHomeLayout } from './TaskHomeLayout';
import { TaskTemplatePanel } from './TaskTemplatePanel';
import { TaskComposer } from './TaskComposer';
import { RunTargetMenu } from './RunTargetMenu';
import { RepositoryChip } from './RepositoryChip';
import { RepositoryPreparationHelp } from './RepositoryPreparationHelp';
import { TaskTemplates } from './TaskTemplates';
import { selectTaskHomePresentation } from './taskHomePresentation';
import { useCrewReplyPreviews } from './useCrewReplyPreviews';
import { AgentLifecycleCard } from './AgentLifecycleCard';
import { useTaskExperience } from '../taskExperience';
import { useTaskDraft } from './taskDraft';
import { taskCreateRequest, useTaskTitleModeSupport } from './taskCreateRequest';
import { useTaskTemplateDraftSwitch } from './TaskTemplateDraftSwitch';
import { useConnectionGate } from '../connections/useConnectionGate';
import { usePreparationDraft } from '../preparation/usePreparationDraft';
import { PreparationDraft } from '../preparation/PreparationDraft';
import { RepositoryConnectionRepair } from '../connections/ConnectionWizard';
import '../ui/Input.css';
import './AgentLifecycleCard.css';
import './TaskHome.css';
import './TaskStartLayout.css';
import './TaskFeedback.css';
const recoverTask = { label: 'Recover original task', kind: 'recover' } as const;
const continueTask = { label: 'Continue original task', kind: 'recover' } as const;
const retryTaskStatus = { label: 'Retry task status', kind: 'status' } as const;
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
  home,
  onRefresh,
}: TaskHomeProps & { readonly onRefresh: () => void }) {
  const policy = useTaskExperience(client, viewerId);
  const titleModeSupported = useTaskTitleModeSupport(client);
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
  const titleId = useId();
  const errorId = useId();
  const templateV1Availability = useTaskTemplateAvailability(client);
  const templateV2Availability = useTaskTemplateAvailability(client, 2);
  const allowTemplateV2 =
    policy.confirmed && policy.task_home_admission && templateV2Availability === 'supported';
  const draft = useTaskDraft(viewerId, workspaceId, sandboxId, allowTemplateV2);
  const template = taskTemplateForSubmission(draft.template, templateV2Availability);
  const input = useRef<HTMLTextAreaElement>(null);
  const [validation, setValidation] = useState<TemplateIssue>();
  const templateAvailability =
    template?.version === 2 ? templateV2Availability : templateV1Availability;
  const templateSupported =
    templateAvailability === 'checking' ? undefined : templateAvailability === 'supported';
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
  const machineIncludedAvailable = Boolean(viewerId && !contextual && canCreateSandbox);
  const lifecycle = useLifecycleIntent(client, viewerId, workspaceId, sandboxId);
  const { intent, operation } = lifecycle;
  const connections = useConnectionGate(!!viewerId, client, undefined, !!intent?.body.repository);
  const preparationDraft = usePreparationDraft(
    client,
    viewerId && intent
      ? { owner: viewerId, intentKey: intent.key, workspaceId, sandboxId }
      : undefined,
  );
  const quotaFailure =
    operation?.status === 'failed' && operation.failure?.code === 'sandbox_limit_reached'
      ? operation.failure
      : undefined;
  const quotaFailed = quotaFailure !== undefined;
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
    if (
      !preparationDraft.bind({
        operationId: operation.id,
        agentId: agent.id,
        sandboxId: agent.sandbox.id,
        workspaceId: agent.workspace.id,
        worktreeId: agent.worktree.id,
      })
    )
      return;
    if (!lifecycle.bindAgent(agent.id)) return;
    opened.current = operation.id;
    handoff();
    onCreated({ kind: 'agent', agentId: agent.id });
  }, [agent, handoff, lifecycle, onCreated, operation, preparationDraft]);
  const submission = taskSubmission(
    { ...draft, template },
    {
      machine: machineIncludedAvailable,
      repository: repositorySupported,
      template: templateSupported,
    },
  );
  const submissionDisabled =
    !policy.task_home_admission ||
    capacityUnavailable ||
    checkingCapacity ||
    lifecycle.pending ||
    lifecycle.blocked ||
    Boolean(intent) ||
    !canCreate ||
    titleModeSupported === undefined ||
    supported !== true;
  const presentation = selectTaskHomePresentation({
    navigation: home?.navigation,
    scope: home?.scope,
    contextual,
    templateSelected: Boolean(draft.template),
    submitted: Boolean(intent),
  });
  const previews = useCrewReplyPreviews({
    client,
    viewerId,
    authEpoch: home?.authEpoch,
    scope: home?.scope,
    navigation: home?.navigation,
    visibleAgentIds: presentation.crew.map(({ agent }) => agent.id),
    visible: home?.visible,
    enabled: !contextual && !draft.template && !intent,
  });
  return (
    <TaskHomeLayout
      titleId={titleId}
      home={home}
      contextual={contextual}
      contextLabel={workspaceId ? `Workspace ${workspaceId}` : `Sandbox ${sandboxId}`}
      template={draft.template}
      submitted={Boolean(intent)}
      onNavigate={home?.onNavigate ?? onCreated}
      previews={previews}
    >
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
      {templateDraft.feedback}
      {!intent ? (
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
            const draftRevision = draft.persist(template);
            if (!draftRevision) return;
            if (!contextual) {
              setCheckingCapacity(true);
              const current = await capacity.ensureFresh();
              if (!mounted.current) return;
              setCheckingCapacity(false);
              if (!current || current.available === 0) return;
            }
            void lifecycle.submit(
              taskCreateRequest(
                { ...draft, template, text: submission.text },
                { workspaceId, sandboxId },
                titleModeSupported,
              ),
              draftRevision,
            );
          }}
        >
          <TaskTemplatePanel
            template={draft.template}
            availability={templateAvailability}
            allowV2={allowTemplateV2}
            issue={validation}
            errorId={errorId}
            onChange={(template) => {
              setValidation(undefined);
              draft.changeTemplate(template);
            }}
          >
            <TaskComposer
              text={submission.text}
              template={Boolean(draft.template)}
              invalid={Boolean(draft.template && submission.issue)}
              input={input}
              validation={validation?.message}
              errorId={errorId}
              disabled={submissionDisabled}
              capacity={
                !contextual ? (
                  <SandboxCapacityPanel
                    client={client}
                    capacity={capacity}
                    presentation="home"
                    onEnvironments={home?.onEnvironments}
                  />
                ) : null
              }
              onClearTemplate={templateDraft.clear}
              onChange={(text) => {
                setValidation(undefined);
                draft.change(text);
              }}
            >
              {presentation.templates === 'menu' ? (
                <TaskTemplates presentation="menu" onSelect={templateDraft.select} />
              ) : null}
              <RunTargetMenu
                workspaceId={workspaceId}
                sandboxId={sandboxId}
                recordedDesktop={false}
                machineIncluded={draft.machineIncluded}
                machineIncludedAvailable={machineIncludedAvailable}
                frozen={false}
                onMachineChange={draft.changeMachineIncluded}
              />
              <span className="task-home__divider" aria-hidden="true" />
              <div
                className="task-home__repository-slot"
                data-has-repository={
                  Boolean(draft.repositoryUrl || draft.repositoryRef) || undefined
                }
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
          </TaskTemplatePanel>
        </form>
      ) : null}
      {presentation.templates === 'expanded' ? (
        <TaskTemplates onSelect={templateDraft.select} />
      ) : null}
      {supported === undefined || titleModeSupported === undefined ? (
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
      <TaskCreationStatus
        titleId={titleId}
        motion={{ client, viewerId, authEpoch: home?.authEpoch, visible: home?.visible }}
        connectionRequired={Boolean(lifecycle.connection)}
        draft={<PreparationDraft draft={preparationDraft} />}
        intent={intent}
        operation={operation}
        failure={failure}
        pending={lifecycle.pending}
        hasFollowUp={Boolean(preparationDraft.text || preparationDraft.error)}
        onDiscardWaiting={
          lifecycle.canDiscardWaiting
            ? () => {
                if (!lifecycle.discardWaiting()) return;
                connections.cancel();
                connections.clearError();
                setValidation(undefined);
                requestAnimationFrame(() => input.current?.focus());
              }
            : undefined
        }
        resolvedCommit={agent?.workspace.resolvedCommit}
        recovery={
          <>
            {intent && !intent.receipt && !lifecycle.connection ? (
              <Button
                variant="outline"
                placement="task-home__action"
                disabled={lifecycle.pending || lifecycle.blocked}
                onClick={() => void lifecycle.submit()}
              >
                {recoverTask.label}
              </Button>
            ) : null}
            {lifecycle.connection && !lifecycle.connection.repository_access ? (
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
            {failure && intent?.receipt ? (
              <Button
                variant="outline"
                placement="task-home__action"
                onClick={lifecycle.retryStatus}
              >
                {retryTaskStatus.label}
              </Button>
            ) : null}
          </>
        }
      />
      {connections.dialog}
      {viewerId && lifecycle.connection?.repository_access ? (
        <RepositoryConnectionRepair
          owner={viewerId}
          {...lifecycle.connection}
          pending={lifecycle.pending || lifecycle.blocked}
          onRecheck={() => void lifecycle.submit()}
        />
      ) : null}
      {quotaFailed ? <TaskQuotaNotice quotaUsage={quotaFailure?.quota_usage} /> : null}
      <RepositoryPreparationHelp
        client={client}
        viewerId={viewerId}
        intent={intent}
        operation={operation}
      />
      {intent?.receipt && operation?.status === 'failed' ? (
        <Button
          disabled={
            (quotaFailed && capacityUnavailable) ||
            Boolean(preparationDraft.text || preparationDraft.error)
          }
          variant="outline"
          placement="task-home__action"
          onClick={() => {
            if (lifecycle.clear()) input.current?.focus();
          }}
        >
          Start another task
        </Button>
      ) : null}
      {intent?.receipt && operation?.status === 'failed' && preparationDraft.text ? (
        <p role="status">
          Keep or copy your follow-up draft before starting another task. Clear it here when you’re
          ready.
        </p>
      ) : null}
      {draft.error ? (
        <p className="task-home__alert" role="alert">
          {draft.error}
        </p>
      ) : null}
      {failure && !intent ? (
        <p className="task-home__alert" role="alert">
          {failure}
        </p>
      ) : null}
      {connections.error ? (
        <p className="task-home__alert" role="alert">
          {connections.error}
        </p>
      ) : null}
    </TaskHomeLayout>
  );
}
