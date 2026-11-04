import type { AgentBrowserControlClient, Cma } from '@chronoai/cma-sdk';
import type { ReadOptions } from '../client';

export function computerClient(sdk: Cma) {
  const pending = new Map<string, Promise<unknown>>();
  const run = <Value>(agent: string, operation: () => Promise<Value>, signal?: AbortSignal) => {
    const controller = new AbortController();
    const abort = () => controller.abort(signal?.reason);
    if (signal?.aborted) abort();
    else signal?.addEventListener('abort', abort, { once: true });
    const deadline = setTimeout(
      () => controller.abort(new DOMException('Computer request timed out', 'TimeoutError')),
      4000,
    );
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
            timeoutMs: Math.min(options?.timeoutMs ?? 4000, 4000),
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
            timeoutMs: Math.min(options?.timeoutMs ?? 4000, 4000),
          }),
        options?.signal,
      ),
  };
  return {
    browserControl,
    environment: (agent: string, options?: ReadOptions) =>
      run(agent, () => sdk.agentFiles.environment(agent, { timeoutMs: 4000 }), options?.signal),
  };
}
