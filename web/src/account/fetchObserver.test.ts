import { afterEach, describe, expect, it, vi } from 'vitest';
import { createAccountDeletionFetchObserver } from './fetchObserver';

afterEach(() => vi.restoreAllMocks());

function problemResponse(code: string, contentType = 'application/problem+json') {
  return new Response(JSON.stringify({ code, detail: 'blocked' }), {
    status: 403,
    headers: { 'content-type': contentType },
  });
}

describe('account deletion fetch observer', () => {
  it('observes same-origin CMA problem responses without consuming the body', async () => {
    const notify = vi.fn();
    const observed = createAccountDeletionFetchObserver(
      vi.fn().mockResolvedValue(problemResponse('account_deletion_in_progress')),
      notify,
    );
    const response = await observed('/api/v1/agents');

    expect(await response.json()).toMatchObject({ code: 'account_deletion_in_progress' });
    await vi.waitFor(() => expect(notify).toHaveBeenCalledOnce());
  });

  it('ignores cross-origin and non-problem errors', async () => {
    const notify = vi.fn();
    const fetchImpl = vi
      .fn()
      .mockImplementationOnce(async () => problemResponse('account_deletion_in_progress'))
      .mockImplementationOnce(async () =>
        problemResponse('account_deletion_in_progress', 'application/json'),
      );
    const observed = createAccountDeletionFetchObserver(fetchImpl, notify);

    const crossOrigin = await observed('https://other.example/api/v1/agents');
    const nonProblem = await observed('/api/v1/agents');

    expect(await crossOrigin.text()).toContain('account_deletion_in_progress');
    expect(await nonProblem.text()).toContain('account_deletion_in_progress');
    await Promise.resolve();
    expect(notify).not.toHaveBeenCalled();
  });

  it('ignores non-CMA paths and statuses other than 403', async () => {
    const notify = vi.fn();
    const observed = createAccountDeletionFetchObserver(
      vi.fn().mockImplementation(
        async () =>
          new Response(JSON.stringify({ code: 'account_deletion_in_progress' }), {
            status: 500,
            headers: { 'content-type': 'application/problem+json' },
          }),
      ),
      notify,
    );

    const nonCma = await observed('/public/status');
    const serverError = await observed('/api/v1/agents');

    expect(await nonCma.text()).toContain('account_deletion_in_progress');
    expect(await serverError.text()).toContain('account_deletion_in_progress');
    expect(notify).not.toHaveBeenCalled();
  });
});
