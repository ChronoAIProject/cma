import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createTranscriptDriver } from './transcript';
import type { TerminalSink } from '../types';

/** A sink that records what a driver put on the screen. */
function recordingSink() {
  const written: string[] = [];
  const finished: string[] = [];
  const failed: string[] = [];
  const sink: TerminalSink = {
    write: (data) => written.push(data),
    finish: (reason) => finished.push(reason),
    fail: (message) => failed.push(message),
  };
  return { sink, written, finished, failed };
}

function respondWith(bytes: Uint8Array) {
  const body = new Uint8Array(bytes.byteLength);
  body.set(bytes);
  vi.stubGlobal(
    'fetch',
    vi.fn(() =>
      Promise.resolve(
        new Response(body.buffer, {
          status: 200,
          headers: { 'Content-Type': 'application/octet-stream' },
        }),
      ),
    ),
  );
}

beforeEach(() => {
  respondWith(new TextEncoder().encode('the session said this'));
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('createTranscriptDriver', () => {
  it('writes the stored transcript and says why it is read-only', async () => {
    const { sink, written, finished } = recordingSink();
    const driver = createTranscriptDriver('sesn-1', 'This session is stopped.');

    driver.attach(sink);
    await vi.waitFor(() => {
      expect(finished).toHaveLength(1);
    });

    expect(written.join('')).toBe('the session said this');
    expect(finished[0]).toBe('This session is stopped.');
  });

  it('drops keystrokes rather than buffering them', async () => {
    const { sink, written } = recordingSink();
    const driver = createTranscriptDriver('sesn-1', 'ended');
    driver.attach(sink);

    driver.send('ls\r');
    driver.resize(200, 60);
    await vi.waitFor(() => {
      expect(written).toHaveLength(1);
    });

    // Nothing typed reaches anywhere, and the geometry the transcript was
    // produced at is not reflowed to a new one.
    expect(written.join('')).toBe('the session said this');
  });

  it('shows the transcript again after being detached and re-attached', async () => {
    const first = recordingSink();
    const driver = createTranscriptDriver('sesn-1', 'ended');
    driver.attach(first.sink);
    await vi.waitFor(() => {
      expect(first.written).toHaveLength(1);
    });
    driver.detach();

    // What the canvas does whenever a node is evicted from the attachment
    // budget and later focused back into it. Coming back blank would look like
    // the session had lost its history.
    const second = recordingSink();
    driver.attach(second.sink);
    await vi.waitFor(() => {
      expect(second.written).toHaveLength(1);
    });

    expect(second.written.join('')).toBe('the session said this');
  });

  it('says so when the history cannot be read', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(() => Promise.reject(new Error('gateway is down'))),
    );
    const { sink, failed } = recordingSink();
    const driver = createTranscriptDriver('sesn-1', 'ended');

    driver.attach(sink);
    await vi.waitFor(() => {
      expect(failed).toHaveLength(1);
    });

    expect(failed[0]).toMatch(/could not read this session's history/i);
  });

  it('writes nothing once detached', async () => {
    const { sink, written } = recordingSink();
    const driver = createTranscriptDriver('sesn-1', 'ended');

    driver.attach(sink);
    driver.detach();
    // Long enough for the in-flight fetch to have resolved into a dropped sink.
    await Promise.resolve();
    await Promise.resolve();

    expect(written).toEqual([]);
  });
});
