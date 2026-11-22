import { act, renderHook, waitFor } from '@testing-library/react';
import { expect, it } from 'vitest';
import { agentClient } from '../test/agent';
import { useTaskTitleModeSupport } from './taskCreateRequest';

it('requires fresh support from a replacement client and ignores a stale completion', async () => {
  let finish!: (supported: boolean) => void;
  const first = agentClient({ readTaskTitleModeSupport: async () => true });
  const pending = agentClient({
    readTaskTitleModeSupport: () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  });
  const legacy = agentClient();
  const { result, rerender } = renderHook(({ client }) => useTaskTitleModeSupport(client), {
    initialProps: { client: first },
  });
  await waitFor(() => expect(result.current).toBe(true));
  rerender({ client: pending });
  expect(result.current).toBeUndefined();
  rerender({ client: legacy });
  expect(result.current).toBe(false);
  await act(async () => finish(true));
  expect(result.current).toBe(false);
});
