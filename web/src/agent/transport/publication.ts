interface Publication<Metadata> {
  readonly publish: (metadata: Metadata) => undefined;
  readonly resolve: () => void;
  readonly reject: (error: unknown) => void;
}

/** Batch completed projections, never authority: each batch starts a fresh check. */
export function createProjectionPublication<Metadata>(
  metadata: Promise<Metadata>,
  authorize: () => Promise<void>,
  guard: () => void,
  signal: AbortSignal,
) {
  let context: { value: Metadata } | undefined;
  let pending: Publication<Metadata>[] = [];
  let active: Publication<Metadata>[] = [];
  let running = false;
  let failure: { error: unknown } | undefined;
  const fail = (error: unknown) => {
    failure = { error };
    for (const item of [...active, ...pending]) item.reject(error);
    pending = [];
  };
  const abort = () => fail(signal.reason);
  signal.addEventListener('abort', abort, { once: true });
  const assertOpen = () => {
    signal.throwIfAborted();
    if (failure) throw failure.error;
    guard();
  };
  const flush = async () => {
    if (running || !context || pending.length === 0 || failure) return;
    running = true;
    // Capture before authorization starts: later data can only join a later check.
    const batch = pending;
    active = batch;
    pending = [];
    try {
      signal.throwIfAborted();
      await authorize();
      for (const item of batch) {
        assertOpen();
        item.publish(context.value);
        item.resolve();
      }
    } catch (error) {
      for (const item of batch) item.reject(error);
      fail(error);
    } finally {
      running = false;
      active = [];
      queueMicrotask(() => void flush());
    }
  };
  void metadata.then((value) => {
    context = { value };
    queueMicrotask(() => void flush());
  }, fail);
  // `undefined` excludes async callbacks: transforms and reads must finish
  // before offering a synchronous publication, never a mutation or stream.
  const offer = (publish: (metadata: Metadata) => undefined): Promise<void> => {
    if (signal.aborted) return Promise.reject(signal.reason);
    if (failure) return Promise.reject(failure.error);
    const result = new Promise<void>((resolve, reject) => {
      pending.push({ publish, resolve, reject });
    });
    queueMicrotask(() => void flush());
    return result;
  };
  return Object.assign(offer, {
    dispose() {
      signal.removeEventListener('abort', abort);
      fail(new DOMException('Projection publication closed.', 'AbortError'));
    },
  });
}
