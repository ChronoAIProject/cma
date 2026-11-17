// @vitest-environment node
import { describe, expect, it } from 'vitest';
import { ProtocolError } from '../index.js';
import { now, transport } from './resource-fixtures.js';

describe('typed AG-UI companions', () => {
  it('reads the Agent-owned hierarchy with nullable first-message and pause state', async () => {
    const t = transport();
    const snapshot = {
      threadId: 'agt_example',
      sandbox: {
        id: 'sbx_example',
        name: 'Project',
        status: 'active',
        paused: false,
        pausedNotice: null,
        rotation: { rotations: 0 },
      },
      workspace: { id: 'wks_example', name: 'Project', path: '/home/cma/project', status: 'ready' },
      worktree: {
        id: 'wkt_example',
        name: 'Review',
        path: '/home/cma/project/.worktrees/review',
        status: 'ready',
      },
      firstMessage: null,
    };
    t.answer(snapshot);
    expect(await t.client.agui.readSandboxSnapshot('agt_example')).toEqual(snapshot);
    expect(t.calls[0]!.url).toContain('/api/v1/agui/agents/agt_example/sandbox');
    await expect(t.client.agui.readSandboxSnapshot('agt_wrong')).rejects.toBeInstanceOf(
      ProtocolError,
    );
    const firstMessage = {
      state: 'completed',
      attempt: 1,
      queuedAt: now,
      completedAt: now,
      failedAt: null,
    };
    t.answer({ ...snapshot, firstMessage });
    expect((await t.client.agui.readSandboxSnapshot('agt_example')).firstMessage).toEqual(
      firstMessage,
    );
  });

  it('reads declared capabilities without inventing unsupported events or mutations', async () => {
    const t = transport();
    const capabilities = {
      protocol: 'ag-ui',
      transports: ['sse'],
      events: { RUN_STARTED: true, TOOL_CALL_ARGS: false },
      features: {
        transport: { streaming: true, resumable: true, websocket: false, pushNotifications: false },
        state: { snapshots: false, deltas: true, persistentState: true },
        humanInTheLoop: {
          supported: true,
          approvals: true,
          interrupts: true,
          approveWithEdits: false,
          interventions: false,
        },
        execution: { codeExecution: true, sandboxed: true },
        custom: {
          'cma:terminal': false,
          'cma:usage': true,
          'cma:liveFrames': true,
          'cma:sandbox': {
            snapshot: true,
            status: true,
            workspace: true,
            worktree: true,
            pausedNotice: true,
            firstMessageDelivery: true,
            mutations: false,
          },
        },
      },
      conformsTo: ['https://spec.chrono-ai.fun/cma/1.0/conf/agui-core'],
    };
    t.answer(capabilities);
    expect(await t.client.agui.readCapabilities()).toEqual(capabilities);
    expect(t.calls[0]!.url).toContain('/api/v1/agui/capabilities');
    t.answer({ ...capabilities, protocol: 'invented' });
    await expect(t.client.agui.readCapabilities()).rejects.toBeInstanceOf(ProtocolError);
  });

  it('uses the same bounded Launch companion transport as the visitor resource', async () => {
    const t = transport();
    t.fetch.mockResolvedValueOnce(
      new Response('', { headers: { 'content-type': 'text/event-stream' } }),
    );
    await t.client.agui.streamLaunchEvents('lch_example', { follow: false, onEvent: () => {} });
    expect(String(t.fetch.mock.calls[0]![0])).toContain('/api/v1/agui/launches/lch_example/events');
  });
});
