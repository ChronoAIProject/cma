import { AgentFileError } from '../agentFiles';

export const PREVIEW_DEADLINE_MS = 4000;

/** Bound the complete optional preview, including body consumption. No retries
 * or retained content: every attempt still passes the current server authority. */
export async function previewDeadline<T>(
  operation: (signal: AbortSignal) => Promise<T>,
  signal?: AbortSignal,
): Promise<T> {
  signal?.throwIfAborted();
  const request = new AbortController();
  let reject: (reason: unknown) => void = () => {};
  const cancelled = new Promise<never>((_resolve, fail) => {
    reject = fail;
  });
  const abort = (reason: unknown) => {
    request.abort(reason);
    reject(reason);
  };
  const onAbort = () => abort(signal?.reason);
  signal?.addEventListener('abort', onAbort, { once: true });
  const timer = setTimeout(
    () =>
      abort(
        new AgentFileError(
          'The file preview took too long. You can retry or choose another file.',
          408,
          'file_preview_timeout',
        ),
      ),
    PREVIEW_DEADLINE_MS,
  );
  try {
    return await Promise.race([operation(request.signal), cancelled]);
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener('abort', onAbort);
  }
}
