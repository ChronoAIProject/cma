import { useEffect, useState } from 'react';
import { ApiError } from '@chronoai/cma-sdk';
import type { CmaWebClient } from '../api/client';
import type { SandboxDetail } from '../api/types';
import { sandboxLifecycleLabel, sandboxRefreshDelay } from '../sandbox/lifecycle';

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
  readonly agentId: string;
  readonly sandboxId: string;
  readonly sandbox?: SandboxDetail;
  readonly stale: boolean;
  readonly observedAt?: string;
}

export function useSandboxInput(
  client: CmaWebClient,
  agentId: string | undefined,
  sandboxId: string | undefined,
): SandboxAvailability {
  const [observation, setObservation] = useState<Observation>();
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
        agentId,
        sandboxId,
        sandbox:
          current?.agentId === agentId && current.sandboxId === sandboxId
            ? current.sandbox
            : undefined,
        stale: true,
        observedAt:
          current?.agentId === agentId && current.sandboxId === sandboxId
            ? current.observedAt
            : undefined,
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
      void client
        .readSandbox(sandboxId, { signal: AbortSignal.any([controller.signal, request.signal]) })
        .then((sandbox) => {
          if (controller.signal.aborted || request.signal.aborted || pending !== request) return;
          if (sandbox.id !== sandboxId)
            throw new Error('Sandbox identity did not match the Agent.');
          if (requestEpoch !== visibilityEpoch || document.hidden) {
            timer = setTimeout(poll, 0);
            return;
          }
          setObservation({
            agentId,
            sandboxId,
            sandbox,
            stale: false,
            observedAt: new Date().toISOString(),
          });
          clearTimeout(expiry);
          expiry = setTimeout(markStale, 10000);
          pollInterval = sandboxRefreshDelay(sandbox);
          timer = setTimeout(poll, pollInterval);
        })
        .catch((error: unknown) => {
          if (controller.signal.aborted || request.signal.aborted || pending !== request) return;
          if (error instanceof ApiError && [401, 403, 404].includes(error.status))
            setObservation({ agentId, sandboxId, stale: true });
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
  }, [agentId, client, sandboxId]);

  const current =
    observation?.agentId === agentId && observation?.sandboxId === sandboxId
      ? observation
      : undefined;
  return {
    inputAvailability: inputAvailability(current),
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

function inputAvailability(observation: Observation | undefined): SandboxInputAvailability {
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
  if (
    sandbox.lifecycle.operation !== undefined ||
    (sandbox.status !== 'active' && sandbox.status !== 'paused')
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
    if (eligibility.reason === 'work_in_progress')
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
