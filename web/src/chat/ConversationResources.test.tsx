import { act, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { AguiTransport } from '../agui';
import type { SessionResourceStatus } from '../sdk';
import { toSession } from '../sdk/sessions';
import { ConversationSurface } from './ConversationSurface';

class ControlledTransport implements AguiTransport {
  opens = 0;
  histories = 0;
  #frame: ((raw: unknown) => void) | undefined;
  #drop: ((why: string) => void) | undefined;

  open(
    _session: string,
    onFrame: (raw: unknown) => void,
    onDrop: (why: string) => void,
  ): () => void {
    this.opens += 1;
    this.#frame = onFrame;
    this.#drop = onDrop;
    return () => {};
  }

  history(): Promise<readonly unknown[]> {
    this.histories += 1;
    return Promise.resolve([]);
  }

  emit(frame: unknown): void {
    this.#frame?.(frame);
  }

  drop(): void {
    this.#drop?.('test drop');
  }
}

function session(resources = true) {
  return toSession({
    id: 'sesn_resources',
    status: 'running',
    kernel: 'codex',
    created_at: '2026-08-23T04:00:00Z',
    conforms_to: [
      'https://spec.chrono-ai.fun/cma/1.0/conf/core',
      'https://spec.chrono-ai.fun/cma/1.0/conf/conversation',
      'https://spec.chrono-ai.fun/cma/1.0/conf/usage',
      ...(resources ? ['https://spec.chrono-ai.fun/cma/1.0/conf/resources'] : []),
    ],
  });
}

async function settle(): Promise<void> {
  await act(async () => {
    for (let turn = 0; turn < 8; turn += 1) {
      await Promise.resolve();
    }
  });
}

const waiting = (): Promise<SessionResourceStatus> => Promise.resolve({ reason: 'no_reading_yet' });

const READING = {
  cpuMillisUsed: null,
  cpuMillisLimit: 1000,
  memoryBytesUsed: 892 * 1024 ** 2,
  memoryBytesLimit: 1024 ** 3,
  memoryBytesPeak: 930 * 1024 ** 2,
  stallPermille: 12,
  ceilingHits: 0,
  allocationFailures: 0,
  kills: 0,
  receivedBytes: 18 * 1024,
  sentBytes: Math.round(8.9 * 1024),
  receivedBytesPerSecond: null,
  sentBytesPerSecond: null,
  intervalMs: null,
} as const;

afterEach(() => {
  vi.useRealTimers();
});

describe('the resource status point read', () => {
  it('reads once on a granted mount, once after recovery, and never polls', async () => {
    vi.useFakeTimers();
    const transport = new ControlledTransport();
    const read = vi.fn(waiting);
    const current = session();
    render(
      <ConversationSurface
        session={current}
        capabilities={current.capabilities}
        transport={transport}
        readResourceStatus={read}
      />,
    );
    await settle();

    expect(read).toHaveBeenCalledTimes(1);
    expect(read).toHaveBeenLastCalledWith('sesn_resources', expect.any(AbortSignal));
    expect(transport.opens).toBe(1);

    act(() => {
      transport.drop();
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(500);
    });
    await settle();

    expect(transport.opens).toBe(2);
    expect(read).toHaveBeenCalledTimes(2);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000);
    });
    expect(read).toHaveBeenCalledTimes(2);
  });

  it('does not ask the route for a session without the grant', async () => {
    const transport = new ControlledTransport();
    const read = vi.fn(waiting);
    const current = session(false);
    render(
      <ConversationSurface
        session={current}
        capabilities={current.capabilities}
        transport={transport}
        readResourceStatus={read}
      />,
    );
    await settle();

    expect(read).not.toHaveBeenCalled();
    expect(screen.getByText('Resources not reported for this session')).toBeInTheDocument();
  });

  it('uses only the documented runtime reason to mark reporting impossible', async () => {
    const transport = new ControlledTransport();
    const read = vi.fn(() =>
      Promise.resolve<SessionResourceStatus>({ reason: 'runtime_does_not_report' }),
    );
    const current = session();
    render(
      <ConversationSurface
        session={current}
        capabilities={current.capabilities}
        transport={transport}
        readResourceStatus={read}
      />,
    );
    await settle();

    expect(
      screen.getByText("This session's runtime cannot report what it is using"),
    ).toBeInTheDocument();
  });
});

describe('the projected state in its reading column', () => {
  it('draws resources after usage and open pressure before the stream', async () => {
    const transport = new ControlledTransport();
    const current = session();
    render(
      <ConversationSurface
        session={current}
        capabilities={current.capabilities}
        transport={transport}
        readResourceStatus={waiting}
      />,
    );
    await settle();

    act(() => {
      transport.emit({
        type: 'STATE_DELTA',
        threadId: current.id,
        runId: 'run_resources',
        'cma:live': true,
        delta: [{ op: 'replace', path: '/resources', value: READING }],
      });
      transport.emit({
        type: 'STATE_DELTA',
        threadId: current.id,
        runId: 'run_resources',
        seq: 8,
        delta: [
          {
            op: 'replace',
            path: '/memoryPressure',
            value: {
              usedBytes: 892 * 1024 ** 2,
              limitBytes: 1024 ** 3,
              occupancyPermille: 871,
              since: '2026-08-23T04:12:00Z',
            },
          },
        ],
      });
    });

    const usage = document.querySelector('.usage');
    const resources = screen.getByLabelText('Resources');
    const pressure = screen.getByRole('alert');
    const stream = screen.getByRole('log', { name: 'Conversation' });
    if (usage === null) {
      throw new Error('the usage line was not mounted');
    }
    expect(usage.compareDocumentPosition(resources) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);
    expect(pressure.compareDocumentPosition(stream) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);
    expect(screen.getByText('Memory under pressure · 892 MiB / 1 GiB')).toBeInTheDocument();
    expect(screen.getByText('CPU · measuring')).toBeInTheDocument();
  });

  it('clears the durable notice and returns the track to neutral on close', async () => {
    const transport = new ControlledTransport();
    const current = session();
    render(
      <ConversationSurface
        session={current}
        capabilities={current.capabilities}
        transport={transport}
        readResourceStatus={waiting}
      />,
    );
    await settle();

    act(() => {
      transport.emit({
        type: 'STATE_DELTA',
        threadId: current.id,
        runId: 'run_resources',
        'cma:live': true,
        delta: [{ op: 'replace', path: '/resources', value: READING }],
      });
      transport.emit({
        type: 'STATE_DELTA',
        threadId: current.id,
        runId: 'run_resources',
        seq: 8,
        delta: [
          {
            op: 'replace',
            path: '/memoryPressure',
            value: {
              usedBytes: 892 * 1024 ** 2,
              limitBytes: 1024 ** 3,
              occupancyPermille: 871,
              since: '2026-08-23T04:12:00Z',
            },
          },
        ],
      });
    });
    expect(screen.getByRole('alert')).toBeInTheDocument();

    act(() => {
      transport.emit({
        type: 'STATE_DELTA',
        threadId: current.id,
        runId: 'run_resources',
        seq: 9,
        delta: [{ op: 'replace', path: '/memoryPressure', value: null }],
      });
    });

    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.getByRole('progressbar', { name: 'Memory' })).not.toHaveAttribute(
      'data-pressure',
    );
  });
});
