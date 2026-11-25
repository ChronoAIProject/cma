import type { CmaWebClient, ReadOptions } from './client';

const sources = new WeakMap<CmaWebClient, CmaWebClient>();

/** Retained views belong to the underlying account client, across read wrappers. */
export function detailReadSource(client: CmaWebClient): CmaWebClient {
  return sources.get(client) ?? client;
}

/** Share only pending reads inside one mounted identity scope; never cache authority. */
export function coalesceDetailReads(client: CmaWebClient) {
  const controllers = new Set<AbortController>();
  const wrap = <T>(read: (id: string, options?: ReadOptions) => Promise<T>) => {
    const pending = new Map<
      string | symbol,
      { controller: AbortController; result: Promise<T>; subscribers: number }
    >();
    return (id: string, options?: ReadOptions): Promise<T> => {
      const signal = options?.signal;
      if (signal?.aborted) return Promise.reject(signal.reason);
      // Bounds/cache options belong to this caller; only plain detail reads coalesce.
      const key = Object.keys(options ?? {}).some((field) => field !== 'signal') ? Symbol(id) : id;
      let entry = pending.get(key);
      if (!entry) {
        const controller = new AbortController();
        controllers.add(controller);
        const result = Promise.resolve().then(() => {
          controller.signal.throwIfAborted();
          return read(id, { ...options, signal: controller.signal });
        });
        entry = { controller, result, subscribers: 0 };
        pending.set(key, entry);
        const settled = entry;
        const clear = () => {
          if (pending.get(key) === settled) pending.delete(key);
          controllers.delete(controller);
        };
        void result.then(clear, clear);
      }
      const shared = entry;
      shared.subscribers += 1;
      return new Promise<T>((resolve, reject) => {
        let finished = false;
        const finish = (complete: () => void) => {
          if (finished) return;
          finished = true;
          signal?.removeEventListener('abort', abort);
          shared.controller.signal.removeEventListener('abort', abort);
          shared.subscribers -= 1;
          if (!shared.subscribers) {
            if (pending.get(key) === shared) pending.delete(key);
            shared.controller.abort();
          }
          complete();
        };
        const abort = () => finish(() => reject(signal?.reason ?? shared.controller.signal.reason));
        signal?.addEventListener('abort', abort, { once: true });
        shared.controller.signal.addEventListener('abort', abort, { once: true });
        void shared.result.then(
          (value) => finish(() => resolve(value)),
          (error: unknown) => finish(() => reject(error)),
        );
      });
    };
  };
  const wrapped: CmaWebClient = {
    ...client,
    readSandbox: wrap(client.readSandbox.bind(client)),
    readWorkspace: wrap(client.readWorkspace.bind(client)),
    readAgent: wrap(client.readAgent.bind(client)),
    readAgentTranscript: wrap(client.readAgentTranscript.bind(client)),
    readAgentUsage: wrap(client.readAgentUsage.bind(client)),
    readAgentResources: wrap(client.readAgentResources.bind(client)),
    readAgentSource: wrap(client.readAgentSource.bind(client)),
    readAgentProvenance: wrap(client.readAgentProvenance.bind(client)),
  };
  sources.set(wrapped, detailReadSource(client));
  return {
    client: wrapped,
    dispose() {
      for (const controller of controllers) controller.abort();
      controllers.clear();
    },
  };
}
