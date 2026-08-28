import { act, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  NO_CAPABILITIES,
  type AguiTransport,
  type Session,
  type SessionResourceStatus,
} from '@chronoai/cma-sdk';
import { stateFrame } from '@chronoai/cma-sdk/testing';
import { ConversationSurface } from './ConversationSurface';

class ControlledTransport implements AguiTransport {
  opens = 0;
  histories = 0;
  #frame: ((raw: unknown) => void) | undefined;
  #drop: ((cause: unknown) => void) | undefined;

  open(
    _session: string,
    _cursor: number,
    onFrame: (raw: unknown) => void,
    onDrop: (cause: unknown) => void,
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

function session(resources = true): Session {
  return {
    id: 'sesn_resources',
    lifecycle: 'running',
    kernel: 'codex',
    createdAt: Date.parse('2026-08-23T04:00:00Z'),
    plane: 'conversation',
    capabilities: {
      ...NO_CAPABILITIES,
      conversation: true,
      usage: true,
      resources,
    },
  };
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
      // Full jitter keeps the 500 ms reconnect delay below this bound.
      await vi.advanceTimersByTimeAsync(600);
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
      transport.emit(
        stateFrame({
          threadId: current.id,
          runId: 'run_resources',
          seq: 7,
          live: true,
          patches: [{ op: 'replace', path: 'resources', value: READING }],
        }),
      );
      transport.emit(
        stateFrame({
          threadId: current.id,
          runId: 'run_resources',
          seq: 8,
          patches: [
            {
              op: 'replace',
              path: 'memoryPressure',
              value: {
                usedBytes: 892 * 1024 ** 2,
                limitBytes: 1024 ** 3,
                occupancyPermille: 871,
                since: '2026-08-23T04:12:00Z',
              },
            },
          ],
        }),
      );
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
      transport.emit(
        stateFrame({
          threadId: current.id,
          runId: 'run_resources',
          seq: 7,
          live: true,
          patches: [{ op: 'replace', path: 'resources', value: READING }],
        }),
      );
      transport.emit(
        stateFrame({
          threadId: current.id,
          runId: 'run_resources',
          seq: 8,
          patches: [
            {
              op: 'replace',
              path: 'memoryPressure',
              value: {
                usedBytes: 892 * 1024 ** 2,
                limitBytes: 1024 ** 3,
                occupancyPermille: 871,
                since: '2026-08-23T04:12:00Z',
              },
            },
          ],
        }),
      );
    });
    expect(screen.getByRole('alert')).toBeInTheDocument();

    act(() => {
      transport.emit(
        stateFrame({
          threadId: current.id,
          runId: 'run_resources',
          seq: 9,
          patches: [{ op: 'replace', path: 'memoryPressure', value: null }],
        }),
      );
    });

    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.getByRole('progressbar', { name: 'Memory' })).not.toHaveAttribute(
      'data-pressure',
    );
  });

  it('draws and clears a durable CPU-pressure episode independently of memory', async () => {
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
      transport.emit(
        stateFrame({
          threadId: current.id,
          runId: 'run_cpu_pressure',
          seq: 10,
          live: true,
          patches: [
            {
              op: 'replace',
              path: 'resources',
              value: { ...READING, cpuMillisUsed: 850 },
            },
          ],
        }),
      );
      transport.emit(
        stateFrame({
          threadId: current.id,
          runId: 'run_cpu_pressure',
          seq: 11,
          patches: [
            {
              op: 'replace',
              path: 'cpuPressure',
              value: {
                usedMillis: 850,
                limitMillis: 1000,
                occupancyPermille: 850,
                since: '2026-08-23T04:12:02Z',
              },
            },
          ],
        }),
      );
    });

    expect(screen.getByRole('alert')).toHaveTextContent('CPU pressure is high');
    expect(screen.getByText('CPU under pressure · 850m / 1 core')).toBeInTheDocument();
    expect(screen.getByRole('progressbar', { name: 'Memory' })).not.toHaveAttribute(
      'data-pressure',
    );

    act(() => {
      transport.emit(
        stateFrame({
          threadId: current.id,
          runId: 'run_cpu_pressure',
          seq: 12,
          patches: [{ op: 'replace', path: 'cpuPressure', value: null }],
        }),
      );
    });

    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.getByRole('progressbar', { name: 'CPU' })).not.toHaveAttribute('data-pressure');
  });
});
