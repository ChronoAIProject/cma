import { describe, expect, it, vi } from 'vitest';
import { createWebSocketDriver } from './websocket';
import type { TerminalSink } from '../types';

/**
 * A socket double.
 *
 * The driver's contract is about which frames go out and what reaches the sink,
 * so the test drives the socket's own callbacks rather than a real connection.
 */
class FakeSocket {
  static readonly OPEN = 1;
  readyState = 1;
  binaryType = '';
  sent: unknown[] = [];
  closed = false;
  onmessage: ((e: MessageEvent) => void) | null = null;
  onerror: (() => void) | null = null;
  onclose: ((e: CloseEvent) => void) | null = null;

  send(data: unknown): void {
    this.sent.push(data);
  }
  close(): void {
    this.closed = true;
  }
  /** Text frames are control messages. */
  control(frame: object): void {
    this.onmessage?.({ data: JSON.stringify(frame) } as MessageEvent);
  }
  /** Binary frames are terminal bytes. */
  output(text: string): void {
    this.onmessage?.({ data: new TextEncoder().encode(text).buffer } as MessageEvent);
  }
}

function harness(openingTurn?: string) {
  const socket = new FakeSocket();
  const sink: TerminalSink & { written: string[]; failures: string[]; finishes: string[] } = {
    written: [],
    failures: [],
    finishes: [],
    write(d) {
      this.written.push(d);
    },
    fail(m) {
      this.failures.push(m);
    },
    finish(r) {
      this.finishes.push(r);
    },
  };
  const driver = createWebSocketDriver({
    url: 'wss://example/terminal',
    openingTurn,
    connect: () => socket as unknown as WebSocket,
  });
  return { socket, sink, driver };
}

function textOf(frame: unknown): string {
  return ArrayBuffer.isView(frame)
    ? new TextDecoder().decode(frame as ArrayBufferView)
    : String(frame);
}

describe('the websocket terminal driver', () => {
  it('renders binary frames as terminal output', () => {
    const { socket, sink, driver } = harness();
    driver.attach(sink);
    socket.output('hello');
    expect(sink.written).toEqual(['hello']);
  });

  it('never renders control frames as output', () => {
    // The split between text and binary is what stops a control message being
    // painted into the user's terminal.
    const { socket, sink, driver } = harness();
    driver.attach(sink);
    socket.control({ t: 'ready', cols: 80, rows: 24 });
    expect(sink.written).toEqual([]);
  });

  it('holds geometry set before ready and sends it once the relay is up', () => {
    // Resizing before the relay is ready must not be lost, or the first frame
    // paints at the wrong size.
    const { socket, sink, driver } = harness();
    driver.attach(sink);
    driver.resize(120, 40);
    expect(socket.sent).toHaveLength(0);
    socket.control({ t: 'ready', cols: 80, rows: 24 });
    expect(socket.sent.map(textOf)).toContain(JSON.stringify({ t: 'resize', cols: 120, rows: 40 }));
  });

  it('sends a later resize immediately', () => {
    const { socket, sink, driver } = harness();
    driver.attach(sink);
    socket.control({ t: 'ready', cols: 80, rows: 24 });
    socket.sent.length = 0;
    driver.resize(100, 30);
    expect(socket.sent.map(textOf)).toEqual([JSON.stringify({ t: 'resize', cols: 100, rows: 30 })]);
  });

  it('submits the opening turn as ordinary typed input', () => {
    // It must be indistinguishable from typing, newline included, or the agent
    // receives a prompt nobody submitted.
    const { socket, sink, driver } = harness('summarise this repo');
    driver.attach(sink);
    socket.control({ t: 'ready', cols: 80, rows: 24 });
    expect(socket.sent.map(textOf)).toContain('summarise this repo\n');
  });

  it('sends keystrokes as binary', () => {
    const { socket, sink, driver } = harness();
    driver.attach(sink);
    driver.send('ls\n');
    // Asserted by content rather than constructor identity: the encoder and
    // the test may not share a realm, which makes instanceof unreliable here.
    expect(textOf(socket.sent[0])).toBe('ls\n');
  });

  it('surfaces a relay error rather than freezing', () => {
    const { socket, sink, driver } = harness();
    driver.attach(sink);
    socket.control({ t: 'error', code: 'forbidden', detail: 'not your session' });
    expect(sink.failures).toEqual(['forbidden: not your session']);
  });

  it('reports an unreadable control frame', () => {
    const { socket, sink, driver } = harness();
    driver.attach(sink);
    socket.onmessage?.({ data: 'not json' } as MessageEvent);
    expect(sink.failures).toHaveLength(1);
  });

  it('ignores control frames it does not know', () => {
    // The protocol may grow; an unknown frame is not a terminal failure.
    const { socket, sink, driver } = harness();
    driver.attach(sink);
    socket.control({ t: 'something-new' });
    socket.control({ t: 'pong' });
    expect(sink.failures).toEqual([]);
    expect(sink.written).toEqual([]);
  });

  it('reports a clean close as finished and an abrupt one as a failure', () => {
    const { socket, sink, driver } = harness();
    driver.attach(sink);
    socket.onclose?.({ wasClean: true, reason: 'session ended' } as CloseEvent);
    expect(sink.finishes).toEqual(['session ended']);

    const second = harness();
    second.driver.attach(second.sink);
    second.socket.onclose?.({ wasClean: false, reason: '' } as CloseEvent);
    expect(second.sink.failures).toHaveLength(1);
  });

  it('stays silent when the close follows a detach', () => {
    // Detaching is the app's own doing; reporting it would show the user an
    // error for switching tabs.
    const { socket, sink, driver } = harness();
    driver.attach(sink);
    driver.detach();
    socket.onclose?.({ wasClean: false, reason: '' } as CloseEvent);
    expect(sink.failures).toEqual([]);
    expect(sink.finishes).toEqual([]);
  });

  it('closes the socket on detach and tolerates a second detach', () => {
    const { socket, sink, driver } = harness();
    driver.attach(sink);
    driver.detach();
    expect(socket.closed).toBe(true);
    expect(() => driver.detach()).not.toThrow();
  });

  it('reports a connection that cannot be opened', () => {
    const sink: TerminalSink & { failures: string[] } = {
      failures: [],
      write() {},
      fail(m) {
        this.failures.push(m);
      },
      finish() {},
    };
    const driver = createWebSocketDriver({
      url: 'wss://example/terminal',
      connect: () => {
        throw new Error('refused');
      },
    });
    driver.attach(sink);
    expect(sink.failures[0]).toContain('refused');
  });

  it('drops input when the socket is not open rather than throwing', () => {
    const { socket, sink, driver } = harness();
    driver.attach(sink);
    socket.readyState = 3;
    expect(() => driver.send('x')).not.toThrow();
    expect(socket.sent).toHaveLength(0);
  });
});

describe('the driver contract', () => {
  it('tolerates attach, detach and reattach', () => {
    const { sink, driver } = harness();
    driver.attach(sink);
    driver.detach();
    expect(() => driver.attach(sink)).not.toThrow();
  });

  it('opens the url it was given', () => {
    const connect = vi.fn(() => new FakeSocket() as unknown as WebSocket);
    const driver = createWebSocketDriver({
      url: 'wss://host/api/v1/sessions/s1/terminal',
      connect,
    });
    driver.attach({ write() {}, fail() {}, finish() {} });
    expect(connect).toHaveBeenCalledWith('wss://host/api/v1/sessions/s1/terminal');
  });
});
