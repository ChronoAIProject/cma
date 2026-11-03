import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { BrowserPanel } from './environment/BrowserPanel';

const context = 'e9ed9ab9-02a9-4c1e-91f5-7e13ca191b24';
const token = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
const frame = () => ({
  agent_id: 'agent-a',
  runtime_incarnation: 'current',
  context_id: context,
  state: 'live',
  location: 'Local page',
  observers: 1,
  captures: 1,
  bytes: 3,
  min_interval_ms: 1000,
  fresh_for_ms: 3000,
  max_frame_bytes: 262144,
  max_observers: 16,
  frame: {
    sequence: 1,
    captured_at: new Date().toISOString(),
    media_type: 'image/jpeg',
    width: 1280,
    height: 720,
    data: '/9j/',
  },
});

it('waits for acquisition receipt, sends real viewport input through the public client, and returns without a task message', async () => {
  const mutations: Record<string, unknown>[] = [];
  let acknowledge: (() => void) | undefined;
  const fetch = vi.fn(async (path: RequestInfo | URL, options?: RequestInit) => {
    if (!String(path).includes('/browser/control')) return Response.json(frame());
    const input = options?.body
      ? (JSON.parse(String(options.body)) as Record<string, unknown>)
      : undefined;
    if (input) mutations.push(input);
    if (input?.command === 'acquire')
      await new Promise<void>((resolve) => {
        acknowledge = resolve;
      });
    const owned = input && input.command !== 'return';
    return Response.json({
      agent_id: 'agent-a',
      runtime_incarnation: 'current',
      context_id: context,
      state: owned ? 'user' : 'agent',
      revision: input ? Number(input.expected_revision) + 1 : 0,
      client_id: owned ? input.client_id : null,
      lease_token: owned ? token : null,
      expires_at: owned ? new Date(Date.now() + 15000).toISOString() : null,
      lease_duration_ms: 15000,
      operation_id: input?.operation_id ?? null,
      operation_revision: input ? Number(input.expected_revision) + 1 : null,
      audit: [],
      audit_truncated_before: null,
    });
  });
  const sdk = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  render(
    <BrowserPanel
      client={sdk.agentBrowser}
      controlClient={sdk.agentBrowserControl}
      controlAccountId="owner-a"
      agentId="agent-a"
      incarnation="current"
      active
      onReload={() => {}}
    />,
  );
  const image = await screen.findByAltText('Agent browser page');
  fireEvent.load(image);
  fireEvent.click(await screen.findByRole('button', { name: 'Take control' }));
  await waitFor(() => expect(acknowledge).toBeDefined());
  expect(screen.queryByText('You are controlling')).not.toBeInTheDocument();
  fireEvent.click(image);
  expect(mutations).toHaveLength(1);
  await act(async () => acknowledge?.());
  expect(await screen.findByText('You are controlling')).toBeInTheDocument();
  vi.spyOn(image, 'getBoundingClientRect').mockReturnValue({
    left: 0,
    top: 0,
    width: 1280,
    height: 720,
  } as DOMRect);
  fireEvent.click(image, { clientX: 100, clientY: 90 });
  await waitFor(() => expect(mutations.at(-1)?.input).toEqual({ kind: 'click', x: 100, y: 90 }));
  fireEvent.change(screen.getByLabelText('Text to type in browser'), {
    target: { value: 'USER-17' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Type text' }));
  await waitFor(() => expect(mutations.at(-1)?.input).toEqual({ kind: 'text', text: 'USER-17' }));
  fireEvent.click(screen.getByRole('button', { name: 'Return to Agent' }));
  await screen.findByText('Agent is controlling');
  const count = mutations.length;
  fireEvent.click(image, { clientX: 100, clientY: 90 });
  expect(mutations).toHaveLength(count);
  expect(mutations.at(-1)?.command).toBe('return');
  expect(mutations.every((value) => value.context_id === context)).toBe(true);
});
