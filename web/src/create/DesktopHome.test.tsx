import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { createRestWebClient } from '../api/rest';
import { CmaClientProvider } from '../api/context';
import { WIRE_AGENT, WIRE_SANDBOX_DETAIL } from '../test/rest';
import { ArtifactPublicationScope } from '../resources/artifactRecovery';
import { AgentSidePanel } from '../agent/AgentSidePanel';
import { TaskHome } from './TaskHome';

afterEach(() => {
  sessionStorage.clear();
  vi.unstubAllGlobals();
});

it('recovers a lost desktop creation receipt after reload into the same task Agent and browser', async () => {
  const task = '  Open the research page\nkeep this exact task  ';
  const requests: { body: unknown; key: string | null }[] = [];
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
    if (path.endsWith('/desktop-capability'))
      return Response.json({
        state: 'available',
        reason: 'verified_runtime',
        runtime_contract: 3,
        min_memory_mib: 2048,
        min_cpu_millis: 1000,
      });
    if (path === '/api/agent-manager/v1/agents' && options?.method === 'POST') {
      requests.push({
        body: JSON.parse(String(options.body)),
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
  const client = createRestWebClient(createClient({ fetch: transport }));
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
  const toggle = await screen.findByRole('checkbox', { name: 'Enable desktop' });
  await waitFor(() => expect(toggle).toBeEnabled());
  expect(toggle).not.toBeChecked();
  fireEvent.click(toggle);
  expect(requests).toHaveLength(0);
  fireEvent.change(screen.getByLabelText('Your task'), { target: { value: task } });
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await screen.findByText('Creation receipt lost');
  first.unmount();
  render(<Journey />);
  expect(screen.getByRole('checkbox', { name: 'Enable desktop' })).toBeChecked();
  expect(screen.getByRole('checkbox', { name: 'Enable desktop' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Recover original task' }));
  await waitFor(() =>
    expect(opened).toHaveBeenCalledExactlyOnceWith({ kind: 'agent', agentId: 'agent-1' }),
  );
  expect(await screen.findByAltText('Agent browser page')).toBeInTheDocument();
  expect(requests).toEqual([
    { body: { name: 'New task', initial_message: task, desktop: true }, key: expect.any(String) },
    { body: { name: 'New task', initial_message: task, desktop: true }, key: requests[0]?.key },
  ]);
  expect(reads).toContain('/api/v1/agents/agent-1/browser');
});
