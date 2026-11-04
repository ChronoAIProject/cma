import { expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { createRestWebClient } from '../rest';

it.each(['read', 'change'] as const)(
  'withdraws a canceled stalled %s and skips canceled queued changes before fresh-authority recovery',
  async (operation) => {
    vi.useFakeTimers();
    try {
      let credential = 'before-rotation';
      const dispatched: { method: string; authorization: string | null }[] = [];
      const scope = {
        runtime_incarnation: 'current',
        context_id: 'context-a',
        client_id: 'client-a',
        client_key: 'local-authority',
        operation_id: 'original-operation',
      };
      const command = { ...scope, command: 'acquire' as const, expected_revision: 0 };
      const client = createRestWebClient(
        createClient({
          baseUrl: 'https://fixture.invalid',
          credential: () => credential,
          fetch: async (_url, options) => {
            dispatched.push({
              method: options?.method ?? 'GET',
              authorization: new Headers(options?.headers).get('authorization'),
            });
            if (dispatched.length === 1) return new Promise<Response>(() => {});
            return Response.json({
              agent_id: 'agent-a',
              runtime_incarnation: 'current',
              context_id: 'context-a',
              state: 'agent',
              revision: 1,
              client_id: null,
              lease_token: null,
              expires_at: null,
              lease_duration_ms: 15000,
              operation_id: scope.operation_id,
              operation_revision: 1,
              audit: [],
              audit_truncated_before: null,
            });
          },
        }),
      );
      const controller = new AbortController();
      let withdrawn = false;
      const stalled = client
        .browserControl![operation]('agent-a', command, { signal: controller.signal })
        .catch(() => {
          withdrawn = true;
        });
      await vi.advanceTimersByTimeAsync(0);
      const queuedController = new AbortController();
      const canceled = client
        .browserControl!.change(
          'agent-a',
          {
            ...command,
            operation_id: 'must-not-dispatch',
          },
          { signal: queuedController.signal },
        )
        .catch(() => {});
      queuedController.abort();
      controller.abort();
      await vi.advanceTimersByTimeAsync(0);
      expect(withdrawn).toBe(true);
      expect(dispatched).toHaveLength(1);
      credential = 'after-rotation';
      await vi.advanceTimersByTimeAsync(3999);
      let recovered = false;
      const recovery = client.browserControl!.read('agent-a', scope).then((value) => {
        expect(value.operation_id).toBe(scope.operation_id);
        recovered = true;
      });
      await vi.advanceTimersByTimeAsync(1);
      expect(recovered).toBe(true);
      expect(dispatched).toEqual([
        { method: operation === 'read' ? 'GET' : 'POST', authorization: 'Bearer before-rotation' },
        { method: 'GET', authorization: 'Bearer after-rotation' },
      ]);
      await Promise.all([stalled, canceled, recovery]);
    } finally {
      vi.useRealTimers();
    }
  },
);

it('waits for environment verification before returning control while a browser read occupies the other permit', async () => {
  let finish: () => void = () => {};
  const pending = new Promise<void>((resolve) => {
    finish = resolve;
  });
  let active = 1;
  const calls: string[] = [];
  const fetch = async (url: RequestInfo | URL, options?: RequestInit) => {
    const environment = String(url).endsWith('/environment');
    calls.push(environment ? 'environment' : 'return');
    active += 1;
    try {
      if (active > 2)
        return Response.json({ status: 429, title: 'file_capacity' }, { status: 429 });
      if (environment) {
        await pending;
        const feature = {
          state: 'available',
          reason: 'supported',
          scope: 'agent_browser',
          writable: true,
        };
        return Response.json({
          agent_id: 'agent-a',
          runtime_incarnation: 'current',
          policy_revision: 1,
          revalidate_at: null,
          files: feature,
          browser: feature,
          control: feature,
          operation_output: feature,
          replay: feature,
        });
      }
      const { client_key, ...body } = command;
      expect(JSON.parse(String(options?.body))).toEqual(body);
      expect(new Headers(options?.headers).get('x-cma-browser-client')).toBe(client_key);
      return Response.json({
        agent_id: 'agent-a',
        runtime_incarnation: 'current',
        context_id: 'context-a',
        state: 'agent',
        revision: 3,
        client_id: null,
        lease_token: null,
        expires_at: null,
        lease_duration_ms: 15000,
        operation_id: 'return-original',
        operation_revision: 3,
        audit: [],
        audit_truncated_before: null,
      });
    } finally {
      active -= 1;
    }
  };
  const client = createRestWebClient(createClient({ baseUrl: 'https://fixture.invalid', fetch }));
  const command = {
    command: 'return' as const,
    runtime_incarnation: 'current',
    context_id: 'context-a',
    client_id: 'client-a',
    client_key: 'local-authority',
    operation_id: 'return-original',
    expected_revision: 2,
    lease_token: 'local-lease',
  };
  const environment = client.files!.environment!('agent-a');
  await expect.poll(() => calls.slice()).toEqual(['environment']);
  const returned = client
    .browserControl!.change('agent-a', command)
    .catch((error: unknown) => error);
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(calls).toEqual(['environment']);
  finish();
  await environment;
  expect(await returned).toMatchObject({ state: 'agent', operation_id: 'return-original' });
  expect(calls).toEqual(['environment', 'return']);
});
