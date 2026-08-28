import { describe, expect, it } from 'vitest';
import { toEvent, toSession } from '../../src/index.js';
import { sessionPayload, stateFrame } from '../../src/testing.js';

describe('consumer fixture builders', () => {
  it('owns the REST session spellings and produces a parseable session', () => {
    const payload = sessionPayload({
      id: 'sesn_fixture',
      grants: ['conversation', 'usage'],
      concurrentInput: 'enqueue',
      cols: 120,
      rows: 32,
      lastSeq: 7,
    });

    expect(payload).toMatchObject({
      created_at: '2026-01-01T00:00:00Z',
      concurrent_input: 'enqueue',
      last_seq: 7,
    });
    expect(toSession(payload)).toMatchObject({
      id: 'sesn_fixture',
      plane: 'conversation',
      cols: 120,
      rows: 32,
      lastSeq: 7,
      capabilities: { conversation: true, usage: true, concurrentInput: 'enqueue' },
    });
  });

  it('owns the state discriminator, live marker, delta field, and pointer paths', () => {
    expect(
      toEvent(
        stateFrame({
          threadId: 'sesn_fixture',
          runId: 'run_fixture',
          seq: 7,
          live: true,
          patches: [{ op: 'replace', path: 'resources', value: { memoryBytesUsed: 10 } }],
        }),
      ),
    ).toMatchObject({
      type: 'STATE_DELTA',
      threadId: 'sesn_fixture',
      runId: 'run_fixture',
      seq: 7,
      ephemeral: true,
      patch: [{ op: 'replace', path: '/resources', value: { memoryBytesUsed: 10 } }],
    });
  });
});
