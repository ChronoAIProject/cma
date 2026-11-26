import { AVAILABLE_CAPACITY } from '../test/capacity';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient } from '@cma/client';
import { createRestWebClient } from '../api/rest';
import { CmaClientProvider } from '../api/context';
import { WIRE_AGENT, WIRE_SANDBOX_DETAIL } from '../test/rest';
import { ArtifactPublicationScope } from '../resources/artifactRecovery';
import { AgentSidePanel } from '../agent/AgentSidePanel';
import { TaskHome } from './TaskHome';
import { taskDraftStorageKey } from './taskDraft';
import { taskExperience } from '../test/taskExperience';

afterEach(() => {
  sessionStorage.clear();
  vi.unstubAllGlobals();
});

it('drops a legacy desktop draft choice without changing its task or blocking submission', async () => {
  sessionStorage.setItem(
    `cma:task-draft:${JSON.stringify([location.origin, 'owner', null, null])}`,
    JSON.stringify({ version: 1, text: 'Retain my draft', desktop: true }),
  );
  const writes: unknown[] = [];
  const transport: typeof fetch = async (input, options) => {
    if (options?.method === 'POST') {
      writes.push(JSON.parse(String(options.body)));
      throw new TypeError('Receipt unavailable');
    }
    if (String(input).endsWith('/openapi.json'))
      return Response.json({
        openapi: '3.1.0',
        info: { title: 'CMA', version: '1' },
        paths: {},
        components: {
          schemas: {
            AgentLifecycleCreateRequest: {
              properties: { initial_message: { type: 'string' }, desktop: { type: 'boolean' } },
            },
          },
        },
      });
    throw new Error(`Unexpected read: ${String(input)}`);
  };
  vi.stubGlobal('fetch', transport);
  const client = {
    ...createRestWebClient(createClient({ fetch: transport })),
    readSandboxCapacity: async () => AVAILABLE_CAPACITY,
    taskExperience: taskExperience('owner'),
  };
  render(<TaskHome client={client} viewerId="owner" canCreateSandbox onCreated={() => {}} />);
  computerOption('Shared desktop');
  expect(screen.queryByRole('menuitemcheckbox', { name: 'Agent browser' })).not.toBeInTheDocument();
  expect(screen.queryByText(/Your saved task could not be read/)).not.toBeInTheDocument();
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
  expect(writes).toHaveLength(0);
  expect(screen.getByLabelText('Your task')).toHaveValue('Retain my draft');
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await screen.findByText(
    "We couldn't confirm your task was received. Use Recover original task to resend the same request.",
  );
  expect(writes).toEqual([{ name: 'New task', initial_message: 'Retain my draft' }]);
  const saved = sessionStorage.getItem(taskDraftStorageKey('owner'));
  expect(JSON.parse(saved!)).not.toHaveProperty('desktop');
});

it('recovers a lost desktop creation receipt after reload into the same task Agent and browser', async () => {
  const task = '  Open the research page\nkeep this exact task  ';
  const body = { name: 'New task', initial_message: task, desktop: true };
  const key = 'recorded-browser-task';
  sessionStorage.setItem(
    `cma:agent-creation:${JSON.stringify([location.origin, 'owner'])}`,
    JSON.stringify({ version: 1, intent: { key, body } }),
  );
  const requests: { body: string; key: string | null }[] = [];
  const reads: string[] = [];
  const transport: typeof fetch = async (input, options) => {
    const path = new URL(String(input), location.origin).pathname;
    reads.push(path);
    if (path.endsWith('/openapi.json'))
      return Response.json({
        openapi: '3.1.0',
        info: { title: 'CMA', version: '1' },
        paths: {},
        components: {
          schemas: {
            AgentLifecycleCreateRequest: {
              properties: { initial_message: { type: 'string' }, desktop: { type: 'boolean' } },
            },
          },
        },
      });
    if (path === '/api/agent-manager/v1/agents' && options?.method === 'POST') {
      requests.push({
        body: String(options.body),
        key: new Headers(options.headers).get('Idempotency-Key'),
      });
      if (requests.length === 1) throw new TypeError('Creation receipt lost');
      return Response.json({ id: 'desktop-operation', status: 'provisioning' }, { status: 202 });
    }
    if (path.endsWith('/agent-lifecycle/desktop-operation'))
      return Response.json({
        id: 'desktop-operation',
        status: 'running',
        phase: 'desktop_preparation',
        agent_id: 'agent-1',
        events: [],
      });
    if (path.endsWith('/agents/agent-1')) return Response.json(WIRE_AGENT);
    if (path.endsWith('/sandboxes/sandbox-1'))
      return Response.json({
        ...WIRE_SANDBOX_DETAIL,
        owner: 'owner',
        lifecycle: { pause: { allowed: false }, revive: { allowed: false } },
      });
    if (path.endsWith('/environment')) {
      const feature = (state: string) => ({
        state,
        reason: 'proof',
        scope: 'agent_browser',
        writable: false,
      });
      return Response.json({
        agent_id: 'agent-1',
        policy_revision: 1,
        runtime_incarnation: 'original-runtime',
        revalidate_at: null,
        files: feature('available'),
        operation_output: feature('available'),
        browser: feature('available'),
        replay: feature('unsupported'),
        control: feature('unsupported'),
      });
    }
    if (path.endsWith('/browser'))
      return Response.json({
        agent_id: 'agent-1',
        runtime_incarnation: 'original-runtime',
        context_id: 'original-browser',
        state: 'live',
        location: 'Local page',
        observers: 1,
        captures: 1,
        bytes: 4,
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
          data: '/9j/AA==',
        },
      });
    throw new Error(`Unexpected request ${path}`);
  };
  vi.stubGlobal('fetch', transport);
  const client = {
    ...createRestWebClient(createClient({ fetch: transport })),
    readSandboxCapacity: async () => AVAILABLE_CAPACITY,
    taskExperience: taskExperience('owner'),
  };
  const opened = vi.fn();
  function Journey() {
    const [agent, setAgent] = useState<string>();
    return (
      <CmaClientProvider client={client}>
        {agent ? (
          <AgentSidePanel
            agentId={agent}
            shared={false}
            panel="browser"
            onClose={() => {}}
            onAccessChanged={() => {}}
            publicationScope={new ArtifactPublicationScope()}
          />
        ) : (
          <TaskHome
            client={client}
            viewerId="owner"
            canCreateSandbox
            onCreated={(route) => {
              opened(route);
              if (route.kind === 'agent') setAgent(route.agentId);
            }}
          />
        )}
      </CmaClientProvider>
    );
  }
  const first = render(<Journey />);
  expect(screen.queryByRole('button', { name: /^Cloud computer/ })).not.toBeInTheDocument();
  expect(screen.getByRole('region', { name: 'Original request' }).textContent).toBe(task);
  expect(requests).toHaveLength(0);
  fireEvent.click(screen.getByRole('button', { name: 'Recover original task' }));
  await screen.findByText(
    "We couldn't confirm your task was received. Use Recover original task to resend the same request.",
  );
  first.unmount();
  render(<Journey />);
  expect(screen.queryByRole('button', { name: /^Cloud computer/ })).not.toBeInTheDocument();
  expect(screen.getByRole('region', { name: 'Original request' }).textContent).toBe(task);
  fireEvent.click(screen.getByRole('button', { name: 'Recover original task' }));
  await waitFor(() =>
    expect(opened).toHaveBeenCalledExactlyOnceWith({ kind: 'agent', agentId: 'agent-1' }),
  );
  expect(await screen.findByAltText('Agent browser page')).toBeInTheDocument();
  expect(requests).toEqual([
    { body: JSON.stringify(body), key },
    { body: JSON.stringify(body), key },
  ]);
  expect(reads).toContain('/api/v1/agents/agent-1/browser');
  expect(reads.some((path) => path.endsWith('/desktop-capability'))).toBe(false);
});

function computerOption(name: string) {
  if (!screen.queryByRole('menu', { name: 'Run this task on' }))
    fireEvent.click(screen.getByRole('button', { name: /^Cloud computer/ }));
  return screen.getByRole('menuitemcheckbox', { name });
}
