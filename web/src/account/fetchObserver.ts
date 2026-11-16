import { notifyAccountDeletionInProgress } from './deletion';

type FetchLike = typeof globalThis.fetch;

function isCmaRequest(input: RequestInfo | URL): boolean {
  const locationOrigin = globalThis.location?.origin;
  if (locationOrigin === undefined) return false;
  const rawUrl =
    input instanceof Request ? input.url : input instanceof URL ? input.href : String(input);
  const url = new URL(rawUrl, globalThis.location.href);
  return (
    url.origin === locationOrigin &&
    (url.pathname.startsWith('/api/') || url.pathname.startsWith('/auth/'))
  );
}

function isProblemResponse(response: Response): boolean {
  return (
    response.status === 403 &&
    (response.headers.get('content-type')?.toLowerCase().startsWith('application/problem+json') ??
      false)
  );
}

export function createAccountDeletionFetchObserver(
  fetchImpl: FetchLike,
  notify: () => void = notifyAccountDeletionInProgress,
): FetchLike {
  return async (input, init) => {
    const response = await fetchImpl(input, init);
    if (isCmaRequest(input) && isProblemResponse(response)) {
      void response
        .clone()
        .json()
        .then((body: unknown) => {
          if (
            typeof body === 'object' &&
            body !== null &&
            (body as { code?: unknown }).code === 'account_deletion_in_progress'
          )
            notify();
        })
        .catch(() => {
          /* The SDK will report the original response error. */
        });
    }
    return response;
  };
}
