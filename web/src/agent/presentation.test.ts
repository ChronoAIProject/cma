import { describe, expect, it } from 'vitest';
import type { AgentRuntimeMetadata } from '../api/types';
import { AGENT_DETAIL, AGENT_TRANSCRIPT } from '../test/agent';
import { workspaceFixture } from '../test/hierarchy';
import { conversationFromTranscript, foldAgentEvent, foldLiveEvent } from './conversation';
import type { AgentResourceState } from './controllerTypes';
import { agentDisplayTitle, presentAgent } from './presentation';

const setup = 'Prepare this empty workspace and wait for the owner.';
const configuredName = 'Research agent';

describe('the shared Agent display title', () => {
  it.each([
    ['current setup', setup],
    ['retained setup after wording changes', 'An earlier setup prompt with different wording.'],
    ['repository setup', 'Prepare the optional repository https://github.com/example/repository'],
  ])('omits only a confirmed %s preview', (_label, description) => {
    const metadata: AgentRuntimeMetadata = {
      description,
      descriptionSource: 'prompt_preview',
      descriptionIsSetupPreview: true,
    };
    expect(agentDisplayTitle({ title: configuredName, runtimeMetadata: metadata })).toBe(
      configuredName,
    );
    expect(metadata.description).toBe(description);
  });

  it('keeps an explicit native title even when it exactly equals setup text', () => {
    expect(
      agentDisplayTitle({
        title: configuredName,
        runtimeMetadata: {
          description: setup,
          descriptionSource: 'provider_title',
          descriptionIsSetupPreview: true,
        },
      }),
    ).toBe(setup);
  });

  it.each([
    { description: setup },
    { description: setup, descriptionIsSetupPreview: true },
    { description: setup, descriptionSource: 'prompt_preview' as const },
    {
      description: setup,
      descriptionSource: 'prompt_preview' as const,
      descriptionIsSetupPreview: false,
    },
  ])('preserves an unknown legacy or unconfirmed description: %j', (runtimeMetadata) => {
    expect(agentDisplayTitle({ title: configuredName, runtimeMetadata })).toBe(setup);
  });

  it('does not treat task wording that resembles setup as a setup message', () => {
    const task = `${setup} Then investigate the bug reported by the owner.`;
    expect(
      agentDisplayTitle({
        title: configuredName,
        runtimeMetadata: { description: task, descriptionSource: 'prompt_preview' },
      }),
    ).toBe(task);
  });

  it('falls back to the configured name and then the existing unnamed label', () => {
    expect(agentDisplayTitle({ title: '  Research agent  ' })).toBe(configuredName);
    expect(agentDisplayTitle({ title: ' ', runtimeMetadata: { description: ' ' } })).toBe(
      'Untitled agent',
    );
  });
});

describe('metadata replay and presentation', () => {
  const blank = () => conversationFromTranscript({ ...AGENT_TRANSCRIPT, throughSeq: 0, turns: [] });
  const event = (seq: number, metadata: Record<string, unknown>) => ({
    seq,
    appendedAt: '2026-09-06T00:01:00Z',
    kind: { type: 'runtime_metadata_changed', metadata },
  });

  it('keeps setup provenance through durable replay and ignores callback/user message positions', () => {
    const recorded = [
      event(1, {
        description: setup,
        description_source: 'prompt_preview',
        description_is_setup_preview: true,
      }),
      {
        seq: 2,
        appendedAt: '2026-09-06T00:02:00Z',
        kind: { type: 'owner_message', content: 'Service access has been updated.' },
      },
      {
        seq: 3,
        appendedAt: '2026-09-06T00:03:00Z',
        kind: { type: 'owner_message', content: 'Investigate the reported issue.' },
      },
    ];
    const replayed = recorded.reduce(foldAgentEvent, blank());
    expect(replayed.messages).toHaveLength(2);
    expect(
      agentDisplayTitle({ title: configuredName, runtimeMetadata: replayed.runtimeMetadata }),
    ).toBe(configuredName);
    expect(recorded.reduce(foldAgentEvent, replayed)).toBe(replayed);
  });

  it('updates from AG-UI and keeps the explicit native title across later fallback replay', () => {
    const preview = foldAgentEvent(
      blank(),
      event(1, {
        description: setup,
        description_source: 'prompt_preview',
        description_is_setup_preview: true,
      }),
    );
    const named = foldLiveEvent(
      preview,
      {
        type: 'STATE_DELTA',
        threadId: 'agt_1',
        runId: '1',
        delta: [
          {
            op: 'replace',
            path: '/runtimeMetadata',
            value: { description: setup, descriptionSource: 'provider_title' },
          },
        ],
      },
      'agt_1',
    );
    expect(named.runtimeMetadata?.descriptionIsSetupPreview).toBeUndefined();
    const resumed = foldAgentEvent(
      named,
      event(2, {
        description: 'Stale automatic setup fallback',
        description_source: 'prompt_preview',
        description_is_setup_preview: true,
        working_directory: '/home/cma/restored',
      }),
    );
    expect(
      agentDisplayTitle({ title: configuredName, runtimeMetadata: resumed.runtimeMetadata }),
    ).toBe(setup);
    expect(resumed.runtimeMetadata).toMatchObject({
      descriptionSource: 'provider_title',
      workingDirectory: '/home/cma/restored',
    });
  });

  it('does not attach old setup provenance to a different unknown description', () => {
    const before = foldAgentEvent(
      blank(),
      event(1, {
        description: setup,
        description_source: 'prompt_preview',
        description_is_setup_preview: true,
      }),
    );
    const after = foldAgentEvent(before, event(2, { description: 'Native legacy title' }));
    expect(after.runtimeMetadata?.descriptionSource).toBeUndefined();
    expect(after.runtimeMetadata?.descriptionIsSetupPreview).toBeUndefined();
    expect(agentDisplayTitle({ runtimeMetadata: after.runtimeMetadata })).toBe(
      'Native legacy title',
    );
  });

  it('keeps saved explicit metadata when a resumed conversation replays an older preview', () => {
    const state: AgentResourceState = {
      status: 'ready',
      detail: {
        ...AGENT_DETAIL,
        runtimeMetadata: { description: 'Native name', descriptionSource: 'provider_title' },
      },
      workspace: workspaceFixture(),
      conversation: {
        ...blank(),
        runtimeMetadata: {
          description: setup,
          descriptionSource: 'prompt_preview',
          descriptionIsSetupPreview: true,
        },
      },
    };
    const resumed = presentAgent(state, 'paused');
    expect(resumed.status).toBe('ready');
    if (resumed.status !== 'ready') throw new Error('Expected ready Agent');
    expect(agentDisplayTitle(resumed.detail)).toBe('Native name');
    expect(resumed.detail.status).toBe('paused');
    expect(resumed.conversation).toBe(state.conversation);
    expect(presentAgent(state)).toBe(state);
  });

  it('retains known provenance when a legacy replay repeats the same preview without its source', () => {
    const state: AgentResourceState = {
      status: 'ready',
      detail: {
        ...AGENT_DETAIL,
        runtimeMetadata: {
          description: setup,
          descriptionSource: 'prompt_preview',
          descriptionIsSetupPreview: true,
        },
      },
      workspace: workspaceFixture(),
      conversation: { ...blank(), runtimeMetadata: { description: setup } },
    };
    expect(presentAgent(state)).toBe(state);
    expect(agentDisplayTitle(state.detail)).toBe(AGENT_DETAIL.title);
  });
});
