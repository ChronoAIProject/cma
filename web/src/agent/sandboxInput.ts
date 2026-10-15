import { useEffect, useMemo, useState } from 'react';
import { ApiError } from '@chronoai/cma-sdk';
import type { CmaWebClient } from '../api/client';
import type { AgentDetail, SandboxDetail } from '../api/types';
import { sandboxLifecycleLabel, sandboxRefreshDelay } from '../sandbox/lifecycle';
import { bootstrapInputAllowed, revivalInputAllowed } from './bootstrap';

export interface SandboxInputAvailability {
  readonly allowed: boolean;
  readonly notice?: string;
  readonly savingStartedAt?: string;
}

export type SandboxResourceAvailability =
  | { readonly state: 'live'; readonly runtimeVersion: string }
  | { readonly state: 'checking' | 'paused' | 'inactive' | 'unavailable'; readonly notice: string };

interface SandboxAvailability {
  readonly inputAvailability: SandboxInputAvailability;
  readonly resourceAvailability: SandboxResourceAvailability;
  readonly progressObservation?: {
    readonly sandbox: SandboxDetail;
    readonly stale: boolean;
    readonly observedAt?: string;
  };
}

interface Observation {
  readonly scope: object;
  readonly agentId: string;
  readonly sandboxId: string;
  readonly sandbox?: SandboxDetail;
  readonly stale: boolean;
  readonly observedAt?: string;
  readonly bootstrapAllowed?: boolean;
  readonly revivalAllowed?: boolean;
}

export function useSandboxInput(
  client: CmaWebClient,
  agentId: string | undefined,
  sandboxId: string | undefined,
  queueWhileStarting = false,
  bootstrap?: AgentDetail['bootstrap'],
  owner?: string,
  queueWhileReviving = false,
): SandboxAvailability {
  const [observation, setObservation] = useState<Observation>();
  const operationId = queueWhileStarting ? bootstrap?.operationId : undefined;
  const workspaceId = queueWhileStarting ? bootstrap?.workspaceId : undefined;
  const bootstrapSandboxId = queueWhileStarting ? bootstrap?.sandboxId : undefined;
  const scope = useMemo(
    () => ({
      client,
      agentId,
      sandboxId,
      operationId,
      workspaceId,
      bootstrapSandboxId,
      owner,
      queueWhileReviving,
    }),
    [
      client,
      agentId,
      sandboxId,
      operationId,
      workspaceId,
      bootstrapSandboxId,
      owner,
      queueWhileReviving,
    ],
  );
  useEffect(() => {
    if (agentId === undefined || sandboxId === undefined) return;
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let expiry: ReturnType<typeof setTimeout> | undefined;
    let pending: AbortController | undefined;
    let deadline: ReturnType<typeof setTimeout> | undefined;
    let visibilityEpoch = 0;
    let pollInterval = 5000;
    const markStale = () => {
      if (controller.signal.aborted) return;
      setObservation((current) => ({
        scope,
        agentId,
        sandboxId,
        sandbox: current?.scope === scope ? current.sandbox : undefined,
        stale: true,
        observedAt: current?.scope === scope ? current.observedAt : undefined,
      }));
    };
    const poll = () => {
      if (controller.signal.aborted || pending !== undefined || document.hidden) return;
      clearTimeout(timer);
      const request = new AbortController();
      pending = request;
      const requestEpoch = visibilityEpoch;
      const timeout = setTimeout(() => {
        if (controller.signal.aborted || pending !== request) return;
        request.abort();
        pending = undefined;
        markStale();
        timer = setTimeout(poll, pollInterval);
      }, 10_000);
      deadline = timeout;
      const signal = AbortSignal.any([controller.signal, request.signal]);
      const parentRead = (async () => client.readSandbox(sandboxId, { signal }))();
      void Promise.all([
        parentRead,
        operationId !== undefined
          ? (async () => client.readAgent(agentId, { signal }))()
          : parentRead.then((sandbox) =>
              queueWhileReviving &&
              owner !== undefined &&
              sandbox.owner === owner &&
              sandbox.status === 'reviving' &&
              sandbox.lifecycle?.operation?.purpose === 'revive'
                ? client.readAgent(agentId, { signal })
                : undefined,
            ),
      ])
        .then(([sandbox, agent]) => {
          if (controller.signal.aborted || request.signal.aborted || pending !== request) return;
          if (sandbox.id !== sandboxId)
            throw new Error('Sandbox identity did not match the Agent.');
          if (
            agent !== undefined &&
            (agent.id !== agentId ||
              agent.sandbox.id !== sandboxId ||
              (workspaceId !== undefined && agent.workspace.id !== workspaceId))
          )
            throw new Error('Bootstrap Agent identity did not match its parent.');
          if (requestEpoch !== visibilityEpoch || document.hidden) {
            timer = setTimeout(poll, 0);
            return;
          }
          setObservation({
            scope,
            agentId,
            sandboxId,
            sandbox,
            stale: false,
            observedAt: new Date().toISOString(),
            bootstrapAllowed:
              agent !== undefined &&
              operationId !== undefined &&
              bootstrapSandboxId === sandboxId &&
              bootstrapInputAllowed(agent, sandbox, owner, operationId),
            revivalAllowed:
              queueWhileReviving &&
              agent !== undefined &&
              revivalInputAllowed(agent, sandbox, owner),
          });
          clearTimeout(expiry);
          expiry = setTimeout(markStale, 10000);
          pollInterval = sandboxRefreshDelay(sandbox);
          timer = setTimeout(poll, pollInterval);
        })
        .catch((error: unknown) => {
          if (controller.signal.aborted || request.signal.aborted || pending !== request) return;
          if (error instanceof ApiError && [401, 403, 404].includes(error.status))
            setObservation({ scope, agentId, sandboxId, stale: true });
          else markStale();
          timer = setTimeout(poll, requestEpoch === visibilityEpoch ? pollInterval : 0);
        })
        .finally(() => {
          clearTimeout(timeout);
          if (pending === request) pending = undefined;
        });
    };
    const visibilityChanged = () => {
      visibilityEpoch += 1;
      clearTimeout(timer);
      markStale();
      poll();
    };
    document.addEventListener('visibilitychange', visibilityChanged);
    poll();
    return () => {
      controller.abort();
      pending?.abort();
      clearTimeout(deadline);
      clearTimeout(timer);
      clearTimeout(expiry);
      document.removeEventListener('visibilitychange', visibilityChanged);
    };
  }, [
    agentId,
    bootstrapSandboxId,
    client,
    operationId,
    owner,
    queueWhileReviving,
    sandboxId,
    scope,
    workspaceId,
  ]);

  const current = observation?.scope === scope ? observation : undefined;
  return {
    inputAvailability: inputAvailability(current, queueWhileStarting),
    resourceAvailability: resourceAvailability(current),
    progressObservation:
      current?.sandbox === undefined
        ? undefined
        : { sandbox: current.sandbox, stale: current.stale, observedAt: current.observedAt },
  };
}

function resourceAvailability(observation: Observation | undefined): SandboxResourceAvailability {
  if (observation === undefined)
    return { state: 'checking', notice: 'Checking Sandbox resources.' };
  if (observation.stale || observation.sandbox?.lifecycle === undefined)
    return {
      state: 'unavailable',
      notice: 'Sandbox status is unavailable. Live resources are held.',
    };
  const sandbox = observation.sandbox;
  if (sandbox.status === 'paused')
    return { state: 'paused', notice: 'Sandbox paused. Live resources resume when it is active.' };
  if (sandbox.status !== 'active' || !sandbox.runtimeVersion?.trim())
    return {
      state: 'inactive',
      notice: 'Live resources are unavailable while the Sandbox runtime is inactive.',
    };
  return { state: 'live', runtimeVersion: sandbox.runtimeVersion };
}

function inputAvailability(
  observation: Observation | undefined,
  queueWhileStarting: boolean,
): SandboxInputAvailability {
  if (observation === undefined) return { allowed: false, notice: 'Checking Sandbox status.' };
  const { sandbox, stale } = observation;
  const savingStartedAt = sandbox?.lifecycle?.checkpoint?.startedAt;
  if (stale)
    return {
      allowed: false,
      notice: 'Sandbox status is unavailable. Reconnecting.',
      savingStartedAt,
    };
  if (sandbox?.lifecycle === undefined)
    return { allowed: false, notice: 'Message availability is not reported.' };
  if (sandbox.lifecycle.checkpoint !== undefined)
    return {
      allowed: false,
      notice: `${sandbox.lifecycle.checkpoint.phase === 'capture' ? 'Saving progress' : 'Finishing save'}. Messages will be available when saving finishes.`,
      savingStartedAt,
    };
  if (sandbox.status === 'failed' || sandbox.status === 'retired')
    return { allowed: false, notice: 'This Sandbox is unavailable. Messages cannot be sent.' };
  if (queueWhileStarting && observation.bootstrapAllowed) return { allowed: true };
  if (observation.revivalAllowed) return { allowed: true };
  if (
    sandbox.lifecycle.operation !== undefined ||
    (sandbox.status !== 'active' && (queueWhileStarting || sandbox.status !== 'paused'))
  )
    return {
      allowed: false,
      notice: `${sandboxLifecycleLabel(sandbox)}. Messages cannot be sent while the Sandbox is changing state.`,
    };
  // Pause itself is not required for messaging; only its reported lifecycle blockers apply.
  const eligibility =
    sandbox.status === 'paused' ? sandbox.lifecycle.revive : sandbox.lifecycle.pause;
  if (!eligibility.allowed) {
    if (eligibility.reason === 'checkpoint_in_progress')
      return {
        allowed: false,
        notice: 'Progress is being saved. Messages will be available when saving finishes.',
      };
    // Starting v2 messages are durably queued, so preparation work need not finish first.
    // Fresh Active status and the operation/checkpoint barriers above still apply.
    if (eligibility.reason === 'work_in_progress' && !queueWhileStarting)
      return {
        allowed: false,
        notice: 'Sandbox work is in progress. Messages will be available when it finishes.',
      };
    if (sandbox.status === 'paused')
      return {
        allowed: false,
        notice: 'This Sandbox cannot resume yet. Messages are unavailable.',
      };
  }
  return { allowed: true };
}
