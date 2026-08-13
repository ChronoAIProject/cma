import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createMockDriver } from './mock';
import type { TerminalSink } from '../types';

function makeSink() {
  const written: string[] = [];
  const sink: TerminalSink = {
    write: (d) => written.push(d),
    fail: vi.fn(),
    finish: vi.fn(),
  };
  return { sink, written, text: () => written.join('') };
}

/** Strip SGR so assertions are about content, not escape codes. */
function plain(s: string): string {
  // eslint-disable-next-line no-control-regex
  return s.replace(/\x1b\[[0-9;]*m/g, '');
}

describe('mock session driver', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('announces the session on attach', () => {
    const { sink, text } = makeSink();
    createMockDriver({ sessionId: 's-0001', kernel: 'codex' }).attach(sink);

    expect(plain(text())).toContain('s-0001');
    expect(plain(text())).toContain('codex');
  });

  it('echoes typed characters', () => {
    const { sink, text } = makeSink();
    const driver = createMockDriver({ sessionId: 's-1', kernel: 'codex' });
    driver.attach(sink);

    driver.send('hi');

    expect(text()).toContain('h');
    expect(text()).toContain('i');
  });

  it('erases on backspace', () => {
    const { sink, written } = makeSink();
    const driver = createMockDriver({ sessionId: 's-1', kernel: 'codex' });
    driver.attach(sink);

    driver.send('ab');
    written.length = 0;
    driver.send('\x7f');

    expect(written.join('')).toBe('\b \b');
  });

  it('does not erase past the start of the line', () => {
    const { sink, written } = makeSink();
    const driver = createMockDriver({ sessionId: 's-1', kernel: 'codex' });
    driver.attach(sink);
    written.length = 0;

    driver.send('\x7f\x7f');

    expect(written.join('')).toBe('');
  });

  it('responds to a submitted turn', () => {
    const { sink, text } = makeSink();
    const driver = createMockDriver({ sessionId: 's-1', kernel: 'codex' });
    driver.attach(sink);

    driver.send('do the thing\r');
    vi.advanceTimersByTime(500);

    expect(plain(text())).toContain('do the thing');
  });

  it('reprompts on an empty line rather than dispatching', () => {
    const { sink, written } = makeSink();
    const driver = createMockDriver({ sessionId: 's-1', kernel: 'codex' });
    driver.attach(sink);
    written.length = 0;

    driver.send('\r');
    vi.advanceTimersByTime(500);

    expect(plain(written.join(''))).not.toContain('dispatching');
  });

  it('replays the prompt carried over from the landing page', () => {
    const { sink, text } = makeSink();
    createMockDriver({
      sessionId: 's-1',
      kernel: 'codex',
      openingTurn: 'build me a web scraper',
    }).attach(sink);

    expect(plain(text())).toContain('build me a web scraper');
  });

  it('does not auto-send the carried-over prompt', () => {
    const { sink, text } = makeSink();
    createMockDriver({
      sessionId: 's-1',
      kernel: 'codex',
      openingTurn: 'rm -rf everything',
    }).attach(sink);
    vi.advanceTimersByTime(1000);

    // Submitting text typed before an auth redirect would be an irreversible
    // action taken on the user's behalf, with no chance to edit it first.
    expect(plain(text())).not.toContain('dispatching');
  });

  it('lets the carried-over prompt be edited before sending', () => {
    const { sink, written } = makeSink();
    const driver = createMockDriver({ sessionId: 's-1', kernel: 'codex', openingTurn: 'abc' });
    driver.attach(sink);
    written.length = 0;

    driver.send('\x7f');

    expect(written.join('')).toBe('\b \b');
  });

  it('ignores input once detached', () => {
    const { sink, written } = makeSink();
    const driver = createMockDriver({ sessionId: 's-1', kernel: 'codex' });
    driver.attach(sink);
    driver.detach();
    written.length = 0;

    driver.send('ignored');

    expect(written).toEqual([]);
  });

  it('cancels pending output on detach', () => {
    const { sink, written } = makeSink();
    const driver = createMockDriver({ sessionId: 's-1', kernel: 'codex' });
    driver.attach(sink);

    driver.send('slow turn\r');
    driver.detach();
    written.length = 0;
    vi.advanceTimersByTime(2000);

    // A timer firing after teardown would write into a disposed terminal.
    expect(written).toEqual([]);
  });

  it('is safe to detach twice', () => {
    const driver = createMockDriver({ sessionId: 's-1', kernel: 'codex' });
    driver.attach(makeSink().sink);

    expect(() => {
      driver.detach();
      driver.detach();
    }).not.toThrow();
  });

  it('accepts resize without a remote runtime', () => {
    const driver = createMockDriver({ sessionId: 's-1', kernel: 'codex' });
    expect(() => {
      driver.resize(120, 40);
    }).not.toThrow();
  });

  it('drops control bytes it cannot meaningfully handle', () => {
    const { sink, written } = makeSink();
    const driver = createMockDriver({ sessionId: 's-1', kernel: 'codex' });
    driver.attach(sink);
    written.length = 0;

    // Arrow keys, with no pty to interpret them.
    driver.send('\x1b[A\x1b[B');

    expect(written.join('')).not.toContain('\x1b[A');
  });
});
