import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { expect, it, vi } from 'vitest';
import { Cma, type Agent } from '@cma/client';
import { createUsageClient } from './client';

const corpus = JSON.parse(
  readFileSync(
    resolve('../api/crates/layer1-public/tests/fixtures/public-contract/agents.json'),
    'utf8',
  ),
) as {
  cases: { id: string; response: { body: Agent } }[];
};
const original = corpus.cases.find((item) => item.id === 'agent-read')!.response.body;

it.each([
  { title: 'Generated', owner_title: 'Owner name', expected: 'Owner name' },
  {
    title: 'Long generated name '.repeat(12).trim(),
    expected: 'Long generated name '.repeat(12).trim(),
  },
  {
    title: '',
    runtime_metadata: {
      description: 'Provider title',
      description_source: 'provider_title' as const,
    },
    expected: 'Provider title',
  },
  {
    title: '',
    runtime_metadata: {
      description: 'First prompt',
      description_source: 'prompt_preview' as const,
    },
    expected: 'First prompt',
  },
  {
    title: '',
    runtime_metadata: {
      description: 'Install tools',
      description_source: 'prompt_preview' as const,
      description_is_setup_preview: true,
    },
    expected: 'Untitled Agent',
  },
  { title: ' ', owner_title: ' ', expected: 'Untitled Agent' },
  { title: original.id, expected: 'Untitled Agent' },
  { title: 'Untitled task', expected: 'Untitled task' },
])(
  'projects only owner Agent name fields with existing title priority: $expected',
  async ({ expected, ...names }) => {
    const agent = {
      ...original,
      owner_title: undefined,
      runtime_metadata: undefined,
      ...names,
    };
    const summary = {
      ...agent,
      sandbox_id: agent.sandbox.id,
      workspace_id: agent.workspace.id,
      worktree_id: agent.worktree.id,
    };
    const transport = vi.fn<typeof fetch>(async (url) => {
      const path = new URL(String(url)).pathname;
      if (path === '/api/v1/agents') return Response.json({ items: [summary], next: 'page-2' });
      if (path === `/api/v1/agents/${agent.id}`) return Response.json(agent);
      throw new Error(`Unexpected request ${path}`);
    });
    const sdk = new Cma({ baseUrl: 'https://cma.invalid', fetch: transport });
    const client = createUsageClient(sdk);
    const signal = new AbortController().signal;
    expect(await client.agents.list({ sandbox: agent.sandbox.id, limit: 50, signal })).toEqual({
      items: [{ id: agent.id, sandboxId: agent.sandbox.id, name: expected }],
      next: 'page-2',
    });
    expect(await client.agents.read(agent.id, { signal })).toEqual({
      id: agent.id,
      sandboxId: agent.sandbox.id,
      name: expected,
    });
    expect(transport).toHaveBeenCalledTimes(2);
    expect(new URL(String(transport.mock.calls[0]![0])).searchParams.get('sandbox')).toBe(
      agent.sandbox.id,
    );
    for (const [, options] of transport.mock.calls)
      expect(options?.signal).toBeInstanceOf(AbortSignal);
  },
);

it('preserves the SDK identity rejection on an exact read', async () => {
  const transport = vi.fn<typeof fetch>(async () =>
    Response.json({ ...original, id: 'other-agent' }),
  );
  const client = createUsageClient(new Cma({ baseUrl: 'https://cma.invalid', fetch: transport }));
  await expect(client.agents.read(original.id, {})).rejects.toThrow(/identity/i);
});
