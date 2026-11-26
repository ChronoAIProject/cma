import { TimeoutError, type AgentBrowserControlClient, type Cma } from '@cma/client';
import type { ReadOptions } from '../client';

/** Browser control keeps its short bound on the serialized per-Agent queue. */
const CONTROL_BUDGET_MS = 4000;
/**
 * An environment read crosses the owner hop and a bounded runtime observation;
 * 4 s cut healthy reads off (#2268). Stay below the 15 s authorized-read guard.
 */
const ENVIRONMENT_BUDGET_MS = 10_000;

export function computerClient(sdk: Cma) {
  const pending = new Map<string, Promise<unknown>>();
  const run = <Value>(
    agent: string,
    operation: () => Promise<Value>,
    signal?: AbortSignal,
    budgetMs = CONTROL_BUDGET_MS,
  ) => {
    const controller = new AbortController();
    const abort = () => controller.abort(signal?.reason);
    if (signal?.aborted) abort();
    else signal?.addEventListener('abort', abort, { once: true });
    // A missed deadline is transient: readers retry instead of reporting the
    // feature as unavailable.
    const deadline = setTimeout(() => controller.abort(new TimeoutError(budgetMs)), budgetMs);
    const result = (pending.get(agent) ?? Promise.resolve())
      .catch(() => {})
      .then(async () => {
        controller.signal.throwIfAborted();
        return operation();
      });
    pending.set(agent, result);
    void result
      .finally(() => {
        if (pending.get(agent) === result) pending.delete(agent);
      })
      .catch(() => {});
    let withdraw: () => void = () => {};
    return new Promise<Value>((resolve, reject) => {
      withdraw = () => reject(controller.signal.reason);
      if (controller.signal.aborted) withdraw();
      else controller.signal.addEventListener('abort', withdraw, { once: true });
      result.then(resolve, reject);
    }).finally(() => {
      clearTimeout(deadline);
      signal?.removeEventListener('abort', abort);
      controller.signal.removeEventListener('abort', withdraw);
    });
  };
  const browserControl: AgentBrowserControlClient = {
    read: (agent, scope, options) =>
      run(
        agent,
        () =>
          sdk.agentBrowserControl.read(agent, scope, {
            ...options,
            signal: undefined,
            timeoutMs: Math.min(options?.timeoutMs ?? CONTROL_BUDGET_MS, CONTROL_BUDGET_MS),
          }),
        options?.signal,
      ),
    change: (agent, input, options) =>
      run(
        agent,
        () =>
          sdk.agentBrowserControl.change(agent, input, {
            ...options,
            signal: undefined,
            timeoutMs: Math.min(options?.timeoutMs ?? CONTROL_BUDGET_MS, CONTROL_BUDGET_MS),
          }),
        options?.signal,
      ),
  };
  return {
    browserControl,
    environment: (agent: string, options?: ReadOptions) =>
      run(
        agent,
        () => sdk.agentFiles.environment(agent, { timeoutMs: ENVIRONMENT_BUDGET_MS }),
        options?.signal,
        ENVIRONMENT_BUDGET_MS,
      ),
  };
}
