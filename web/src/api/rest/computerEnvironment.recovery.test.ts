import { afterEach, expect, it, vi } from 'vitest';
import { TransportError, createClient } from '@cma/client';
import { createRestWebClient } from '../rest';

const feature = {
  state: 'available',
  reason: 'authorized',
  scope: 'mounted_home',
  writable: true,
};
const environment = (revision: number) => ({
  agent_id: 'agent-a',
  runtime_incarnation: 'current',
  policy_revision: revision,
  revalidate_at: null,
  files: feature,
  browser: feature,
  control: feature,
  operation_output: feature,
  replay: feature,
});

afterEach(() => vi.useRealTimers());

it.each(['late success', 'deadline'] as const)(
  'withdraws a canceled environment observer before %s and recovers with a fresh read',
  async (completion) => {
    vi.useFakeTimers();
    let release!: (response: Response) => void;
    let transportSignal: AbortSignal | undefined;
    const fetch = vi.fn(async (_url: RequestInfo | URL, options?: RequestInit) => {
      if (fetch.mock.calls.length === 1) {
        transportSignal = options?.signal ?? undefined;
        return new Promise<Response>((resolve) => {
          release = resolve;
        });
      }
      return Response.json(environment(2));
    });
    const client = createRestWebClient(createClient({ baseUrl: 'https://fixture.invalid', fetch }));
    const scope = new AbortController();
    const published = vi.fn();
    const withdrawn = vi.fn();
    const stale = client.files!.environment!('agent-a', { signal: scope.signal }).then(
      published,
      withdrawn,
    );
    await vi.advanceTimersByTimeAsync(0);
    expect(fetch).toHaveBeenCalledTimes(1);
    scope.abort();
    await stale;
    expect(withdrawn).toHaveBeenCalledExactlyOnceWith(
      expect.objectContaining({ name: 'AbortError' }),
    );
    expect(published).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(9999);
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(transportSignal?.aborted).toBe(false);
    const fresh = client.files!.environment!('agent-a');
    if (completion === 'late success') release(Response.json(environment(1)));
    else await vi.advanceTimersByTimeAsync(1);
    await expect(fresh).resolves.toMatchObject({ policy_revision: 2 });
    expect(fetch).toHaveBeenCalledTimes(2);
    expect(transportSignal?.aborted).toBe(completion === 'deadline');
    if (completion === 'deadline') release(Response.json(environment(1)));
    await vi.advanceTimersByTimeAsync(0);
    expect(published).not.toHaveBeenCalled();
    expect(withdrawn).toHaveBeenCalledTimes(1);
  },
);

it('returns a healthy environment read that outlasts the browser control bound', async () => {
  vi.useFakeTimers();
  const fetch = vi.fn(
    () =>
      new Promise<Response>((resolve) => {
        setTimeout(() => resolve(Response.json(environment(3))), 6000);
      }),
  );
  const client = createRestWebClient(createClient({ baseUrl: 'https://fixture.invalid', fetch }));
  const read = client.files!.environment!('agent-a');
  const settled = vi.fn();
  void read.then(settled, settled);
  await vi.advanceTimersByTimeAsync(5999);
  expect(settled).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  await expect(read).resolves.toMatchObject({ policy_revision: 3 });
});

it('reports a missed environment budget as a retryable timeout, not unavailability', async () => {
  vi.useFakeTimers();
  const fetch = vi.fn(() => new Promise<Response>(() => {}));
  const client = createRestWebClient(createClient({ baseUrl: 'https://fixture.invalid', fetch }));
  const read = client.files!.environment!('agent-a');
  const failure = read.catch((error: unknown) => error);
  await vi.advanceTimersByTimeAsync(9999);
  await vi.advanceTimersByTimeAsync(1);
  const error = await failure;
  expect(error).toBeInstanceOf(TransportError);
  expect(error).toMatchObject({ name: 'TimeoutError', isRetryable: true });
});
