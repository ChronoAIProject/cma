import { useEffect, useState } from 'react';
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
}

interface Observation {
  readonly agentId: string;
  readonly sandboxId: string;
  readonly sandbox?: SandboxDetail;
  readonly stale: boolean;
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
    let inFlight = false;
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
      }));
    };
    const poll = () => {
      if (controller.signal.aborted || inFlight || document.hidden) return;
      clearTimeout(timer);
      inFlight = true;
      const requestEpoch = visibilityEpoch;
      void client
        .readSandbox(sandboxId, { signal: controller.signal })
        .then((sandbox) => {
          if (controller.signal.aborted) return;
          if (sandbox.id !== sandboxId)
            throw new Error('Sandbox identity did not match the Agent.');
          if (requestEpoch !== visibilityEpoch || document.hidden) {
            timer = setTimeout(poll, 0);
            return;
          }
          setObservation({ agentId, sandboxId, sandbox, stale: false });
          clearTimeout(expiry);
          expiry = setTimeout(markStale, 10000);
          pollInterval = sandboxRefreshDelay(sandbox);
          timer = setTimeout(poll, pollInterval);
        })
        .catch(() => {
          if (controller.signal.aborted) return;
          markStale();
          timer = setTimeout(poll, requestEpoch === visibilityEpoch ? pollInterval : 0);
        })
        .finally(() => {
          inFlight = false;
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
      notice: `${sandboxLifecycleLabel(sandbox)}. Messages will be available when saving finishes.`,
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
