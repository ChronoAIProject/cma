import { act, renderHook, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { ReactNode } from 'react';
import type { AgentSource } from '@chronoai/cma-sdk';
import { CmaClientProvider } from '../api/context';
import { unavailableClient } from '../api/client';
import type { MenuEntry } from '../ui/Menu';
import { agentOriginEntries, useAgentSource } from './source';

const value: AgentSource = {
  agent_id: 'agt_one',
  origin: { type: 'trigger', trigger_id: 'trg_one', launch_id: 'lch_one' },
  trigger: { title: 'Project introduction', public_id: 'pub_one', revision: 2, quick_messages: [] },
};

/** Each row as the person reads it: the heading text, or a link's label and target. */
function rows(entries: readonly MenuEntry[]): string[] {
  return entries.map((entry) =>
    'type' in entry
      ? entry.type === 'label'
        ? entry.text
        : entry.type
      : `${String(entry.label)} -> ${entry.href ?? 'no link'}`,
  );
}

it.each([
  ['manual', 'Created manually'],
  ['trigger', 'Created by Trigger'],
  ['channel_bridge', 'Created by channel bridge'],
  ['legacy_unknown', 'Creation origin unknown'],
] as const)('keeps %s creation separate from a later published Trigger', (origin, label) => {
  const entries = agentOriginEntries(
    {
      status: 'ready',
      agentId: 'agt_one',
      value: { ...value, origin: { type: 'workspace_profile' } },
    },
    {
      status: 'ready',
      agentId: 'agt_one',
      value: {
        agent_id: 'agt_one',
        created_at: '2026-09-06T00:00:00Z',
        creation_origin: origin,
      },
    },
  );
  expect(rows(entries)).toEqual([label, 'Published Trigger: Project introduction -> /t/pub_one']);
  expect(rows(entries).join(' ')).not.toContain('trg_one');
});

it('does not infer creation when provenance is unavailable, even with a Trigger shortcut', () => {
  expect(
    rows(
      agentOriginEntries(
        { status: 'ready', agentId: 'agt_one', value },
        { status: 'unavailable', agentId: 'agt_one' },
      ),
    ),
  ).toEqual([
    'Creation origin unavailable',
    'Published Trigger: Project introduction -> /t/pub_one',
  ]);
});

it('prefers the retained owner label and encodes the Trigger link', () => {
  expect(
    rows(
      agentOriginEntries(
        {
          status: 'ready',
          agentId: 'agt_one',
          value: { ...value, trigger: { ...value.trigger!, public_id: 'pub one/two' } },
        },
        { status: 'loading', agentId: 'agt_one' },
        'Created manually',
      ),
    ),
  ).toEqual(['Created manually', 'Published Trigger: Project introduction -> /t/pub%20one%2Ftwo']);
});

it('keeps creation visible without privileged shortcuts and hides mismatched Agent links', () => {
  const provenance = {
    status: 'ready' as const,
    agentId: 'agt_one',
    value: {
      agent_id: 'agt_one',
      created_at: '2026-09-06T00:00:00Z',
      creation_origin: 'trigger' as const,
    },
  };
  expect(
    rows(
      agentOriginEntries(
        {
          status: 'ready',
          agentId: 'agt_one',
          value: {
            access: 'shared',
            agent_id: 'agt_one',
            origin: { type: 'trigger' },
          },
        },
        provenance,
      ),
    ),
  ).toEqual(['Created by Trigger']);
  expect(
    rows(agentOriginEntries({ status: 'unavailable', agentId: 'agt_one' }, provenance)),
  ).toEqual(['Created by Trigger']);
  expect(
    rows(
      agentOriginEntries(
        { status: 'ready', agentId: 'agt_two', value: { ...value, agent_id: 'agt_two' } },
        provenance,
      ),
    ),
  ).toEqual(['Created by Trigger']);
});

it('does not leak an earlier Agent source while the selected Agent is changing', async () => {
  let release!: (value: AgentSource) => void;
  const readAgentSource = vi
    .fn()
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          release = resolve;
        }),
    )
    .mockResolvedValue({ agent_id: 'agt_two', origin: { type: 'manual' } });
  const client = { ...unavailableClient, readAgentSource };
  const wrapper = ({ children }: { children: ReactNode }) => (
    <CmaClientProvider client={client}>{children}</CmaClientProvider>
  );
  const { result, rerender } = renderHook(({ id }) => useAgentSource(id, true), {
    initialProps: { id: 'agt_one' },
    wrapper,
  });
  rerender({ id: 'agt_two' });
  await waitFor(() => expect(result.current.status).toBe('ready'));
  await act(async () => release(value));
  expect(result.current.agentId).toBe('agt_two');
  expect(result.current.status === 'ready' && result.current.value.origin.type).toBe('manual');
});

it('keeps source failures independent and removes previously loaded actions on access loss', async () => {
  const readAgentSource = vi
    .fn()
    .mockResolvedValueOnce(value)
    .mockRejectedValue(new Error('Denied'));
  const client = { ...unavailableClient, readAgentSource };
  const wrapper = ({ children }: { children: ReactNode }) => (
    <CmaClientProvider client={client}>{children}</CmaClientProvider>
  );
  const { result } = renderHook(() => useAgentSource('agt_one', true), { wrapper });
  await waitFor(() => expect(result.current.status).toBe('ready'));
  await act(async () => document.dispatchEvent(new Event('visibilitychange')));
  await waitFor(() => expect(result.current.status).toBe('unavailable'));
  expect(result.current).not.toHaveProperty('value');
});
