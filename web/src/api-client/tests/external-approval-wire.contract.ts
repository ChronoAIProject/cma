// @vitest-environment node
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { expect, it } from 'vitest';
import { conversationFromTranscript } from '../../agent/conversation';
import { withApprovalProjection } from '../../agent/transport/externalApprovalProjection';
import { foldV2Frame, foldV2Item } from '../../agent/transport/projection';
import { ApiError, createClient, type V2AgentEventPage } from '../index';

interface Projection {
  buffered: boolean;
  fraction_ms: number;
  response_headers: Record<string, string>;
  head_headers: Record<string, string>;
  terminal_headers: Record<string, string>;
  stream_headers: Record<string, string>;
  waiting_head: unknown;
  waiting_snapshot: { agent_id: string };
  terminal_snapshot: unknown;
  resumed_stream: string;
}

interface HttpRefusal {
  buffered: boolean;
  snapshot: boolean;
  status: number;
  content_type: string;
  body: unknown;
}

interface StreamRefusal {
  body: string;
  accepted_cursors: string[];
}

const capabilityError = 'external_approval_capability_required';
const options = [
  'run',
  '--success-output',
  'immediate',
  '--color',
  'never',
  '-E',
  'package(=layer1-public) and test(agent_v2::external_approval::)',
];
const archived = process.env.CMA_NEXTEST_REUSE === '1';
const producer = spawnSync(
  archived ? 'bash' : 'cargo',
  archived
    ? ['../.github/scripts/backend-nextest.sh', ...options]
    : ['nextest', ...options, '--locked', '-p', 'layer1-public', '--no-tests', 'fail'],
  {
    cwd: fileURLToPath(new URL('../../../../api/', import.meta.url)),
    encoding: 'utf8',
    maxBuffer: 4 * 1024 * 1024,
    timeout: 20 * 60 * 1000,
  },
);
if (producer.error) throw producer.error;
const output = producer.stdout + '\n' + producer.stderr;
if (producer.status !== 0) throw new Error(`CMA producer tests failed:\n${output}`);

function emitted<T>(name: string): T[] {
  const marker = `EXTERNAL_APPROVAL_${name} `;
  return output
    .split('\n')
    .map((line) => line.trimStart())
    .filter((line) => line.startsWith(marker))
    .map((line) => JSON.parse(line.slice(marker.length)) as T);
}

const projections = emitted<Projection>('PRODUCER_FIXTURE');
const httpRefusals = emitted<HttpRefusal>('HTTP_REFUSAL');
const streamRefusals = emitted<StreamRefusal>('STREAM_REFUSAL');
const preference = 'cma-external-approval-v1';

it('receives every producer case', () => {
  expect(projections.map(({ buffered, fraction_ms }) => [buffered, fraction_ms])).toEqual([
    [false, 0],
    [false, 1],
    [false, 10],
    [false, 100],
    [true, 0],
    [true, 1],
    [true, 10],
    [true, 100],
  ]);
  expect(httpRefusals.map(({ buffered, snapshot }) => [buffered, snapshot])).toEqual([
    [false, true],
    [false, false],
    [true, true],
    [true, false],
  ]);
  expect(streamRefusals).toHaveLength(1);
});

it.each(projections)(
  'replays the timeout for buffered=$buffered at +$fraction_ms ms',
  async (testCase) => {
    const { response_headers: headers } = testCase;
    const agent = testCase.waiting_snapshot.agent_id;
    const responses = [
      Response.json(testCase.waiting_head, { headers: testCase.head_headers }),
      Response.json(testCase.waiting_snapshot, { headers }),
      Response.json(testCase.terminal_snapshot, { headers: testCase.terminal_headers }),
      new Response(testCase.resumed_stream, { headers: testCase.stream_headers }),
    ];
    const api = createClient({
      fetch: async (url, init) => {
        expect(init?.method ?? 'GET').toBe('GET');
        expect(new Headers(init?.headers).get('Prefer')).toBe(preference);
        const response = responses.shift();
        if (!response) throw new Error(`Unexpected request to ${String(url)}`);
        return response;
      },
    }).agentEventsV2;

    const head = await api.head(agent, { externalApprovals: true });
    const pending = await api.snapshot(agent, { externalApprovals: true });
    expect(head.externalApprovalProjection).toEqual(pending.externalApprovalProjection);
    expect(head.externalApprovalProjection?.canReview).toBe(true);
    expect(head.through_cursor).toBe(pending.through_cursor);
    const fresh = (page: V2AgentEventPage) => {
      const projection = page.externalApprovalProjection;
      if (!projection) throw new Error('Missing external approval negotiation');
      return withApprovalProjection(
        {
          ...conversationFromTranscript({ agentId: agent, throughSeq: 0, turns: [] }),
          agentIncarnation: page.incarnation,
        },
        projection,
      );
    };
    const waiting = pending.items.reduce(foldV2Item, fresh(pending));
    expect(waiting.approvals).toEqual([expect.objectContaining({ source: 'external' })]);
    expect(waiting.approvals[0]?.decision).toBeUndefined();
    expect(waiting.responseBinding).toMatchObject({
      commandId: head.active_turn?.response_binding?.command_id,
      responseId: head.active_turn?.response_binding?.response_id,
    });
    const turn = waiting.activeRunId;
    if (!turn) throw new Error('Missing active turn');
    const requested = pending.items.find(
      ({ kind }) => kind?.type === 'external_approval_requested',
    )?.kind;
    if (requested?.type !== 'external_approval_requested') throw new Error('Missing wait event');
    expect(requested.waiting_since).toMatch(
      new RegExp(`\\.${String(testCase.fraction_ms).padStart(3, '0')}Z$`),
    );

    const terminal = await api.snapshot(agent, { externalApprovals: true });
    const replayed = terminal.items.reduce(foldV2Item, fresh(terminal));
    expect(replayed.activeRunId).toBeUndefined();
    expect(replayed.turnOutcomes?.[turn]).toBe('failed');
    expect(replayed.approvals[0]?.decision).toBe('timed_out');
    expect(replayed.messages).toContainEqual(
      expect.objectContaining({ content: 'Synthetic partial answer' }),
    );
    let streamed = waiting;
    const cursors: string[] = [];
    await api.events(agent, {
      externalApprovals: true,
      afterCursor: pending.through_cursor,
      follow: false,
      onExternalApprovalProjection: (projection) => {
        expect(projection).toEqual(terminal.externalApprovalProjection);
        streamed = withApprovalProjection(streamed, projection);
      },
      onEvent: (frame) => {
        streamed = foldV2Frame(streamed, frame, agent);
      },
      onCursor: (cursor) => {
        cursors.push(cursor);
      },
    });
    expect(streamed).toEqual(replayed);
    expect(cursors).toEqual(
      terminal.items
        .filter(({ sequence }) => sequence > waiting.cursor)
        .map(({ cursor }) => cursor),
    );
    expect(cursors.at(-1)).toBe(terminal.through_cursor);
    expect(responses).toHaveLength(0);
  },
);

it.each(httpRefusals)(
  'recognizes pre-open refusal for buffered=$buffered snapshot=$snapshot',
  async ({ snapshot, status, content_type, body }) => {
    const api = createClient({
      fetch: async () => Response.json(body, { status, headers: { 'content-type': content_type } }),
    }).agentEventsV2;
    const agent = 'agt_fixture';
    const cursors: string[] = [];
    const request = snapshot
      ? api.snapshot(agent)
      : api.events(agent, {
          onEvent: () => expect.unreachable('Refusal emitted an event'),
          onCursor: (cursor) => {
            cursors.push(cursor);
          },
        });
    await expect(request).rejects.toBeInstanceOf(ApiError);
    await expect(request).rejects.toMatchObject({ status: 409, code: capabilityError });
    expect(cursors).toEqual([]);
  },
);

it.each(streamRefusals)(
  'keeps the last accepted cursor when the open stream refuses',
  async ({ body, accepted_cursors }) => {
    const api = createClient({
      fetch: async () => new Response(body, { headers: { 'content-type': 'text/event-stream' } }),
    }).agentEventsV2;
    const received: string[] = [];
    const cursors: string[] = [];
    await expect(
      api.events('agt_fixture', {
        onEvent: (frame) => {
          received.push(frame.type);
        },
        onCursor: (cursor) => {
          cursors.push(cursor);
        },
      }),
    ).rejects.toThrow(capabilityError);
    expect(received).toEqual(['cma.agent.event', 'cma.agent.event', 'cma.agent.error']);
    expect(cursors).toEqual(accepted_cursors);
  },
);
