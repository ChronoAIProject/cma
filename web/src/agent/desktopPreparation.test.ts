import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { prepareDesktop, takePreparedDesktop } from './desktopPreparation';
import { desktopSocket, DesktopSocket, stubDesktopSockets } from './desktopPreparation.fixture';

beforeEach(() => {
  vi.useFakeTimers();
  stubDesktopSockets();
});
afterEach(() => {
  vi.runAllTimers();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

function setup() {
  const scope = { context: {}, owner: 'alice' };
  const preparation = prepareDesktop(scope.context, 'agt/one', scope.owner)!;
  return { scope, preparation, socket: desktopSocket() };
}

it.each([0, 1])(
  'hands off a socket in state %s with buffered greetings once and in order',
  (state) => {
    const { scope, preparation, socket } = setup();
    expect(socket.url).toContain('/agents/agt%2Fone/desktop/ws');
    expect(socket.binaryType).toBe('arraybuffer');
    const greeting = [new Uint8Array([82, 70]).buffer, new Uint8Array([66, 32]).buffer];
    socket.readyState = state;
    greeting.forEach((frame) => socket.receive(frame));
    expect(socket.send).not.toHaveBeenCalled();
    expect(takePreparedDesktop(scope, 'agt/one')).toBeUndefined();
    preparation.handoff();
    preparation.release();
    const taken = takePreparedDesktop(scope, 'agt/one')!;
    expect(taken.socket).toBe(socket);
    const received: unknown[] = [];
    socket.onmessage = (event) => received.push(event.data);
    taken.replay();
    taken.replay();
    const next = new Uint8Array([48]).buffer;
    socket.receive(next);
    expect(received).toEqual([...greeting, next]);
    expect(takePreparedDesktop(scope, 'agt/one')).toBeUndefined();
    expect(vi.getTimerCount()).toBe(0);
    document.dispatchEvent(new Event('visibilitychange'));
    expect(socket.close).not.toHaveBeenCalled();
  },
);

it.each(['owner', 'Agent', 'closed', 'closing', 'hidden'] as const)(
  'discards a handed-off connection on %s mismatch and leaves normal connection available',
  (change) => {
    const { scope, preparation, socket } = setup();
    preparation.handoff();
    if (change === 'hidden') vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
    socket.readyState = change === 'closed' ? 3 : change === 'closing' ? 2 : 1;
    expect(
      takePreparedDesktop(
        { ...scope, owner: change === 'owner' ? 'bob' : scope.owner },
        change === 'Agent' ? 'agt/two' : 'agt/one',
      ),
    ).toBeUndefined();
    expect(socket.close).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(0);
    expect(takePreparedDesktop(scope, 'agt/one')).toBeUndefined();
  },
);

it('cannot consume another client and expires unused transferred connections', () => {
  const { scope, preparation, socket } = setup();
  preparation.handoff();
  preparation.release();
  expect(takePreparedDesktop({ context: {}, owner: scope.owner }, 'agt/one')).toBeUndefined();
  expect(socket.close).not.toHaveBeenCalled();
  vi.advanceTimersByTime(10_000);
  expect(socket.close).toHaveBeenCalledOnce();
  expect(takePreparedDesktop(scope, 'agt/one')).toBeUndefined();
});

it.each(['cancel', 'error', 'visibility', 'replace'] as const)('discards on %s', (change) => {
  const { scope, preparation, socket } = setup();
  if (change === 'cancel') preparation.release();
  if (change === 'error') socket.fail();
  if (change === 'visibility') document.dispatchEvent(new Event('visibilitychange'));
  if (change === 'replace') prepareDesktop(scope.context, 'agt/two', 'alice')?.release();
  expect(socket.close).toHaveBeenCalledOnce();
  preparation.handoff();
  expect(takePreparedDesktop(scope, 'agt/one')).toBeUndefined();
  expect(vi.getTimerCount()).toBe(0);
});

it.each(['bytes', 'frames', 'text'] as const)('bounds the greeting buffer by %s', (bound) => {
  const { scope, preparation, socket } = setup();
  preparation.handoff();
  if (bound === 'bytes') socket.receive(new ArrayBuffer(64 * 1024 + 1));
  if (bound === 'text') socket.receive('unexpected text');
  if (bound === 'frames')
    for (let index = 0; index < 17; index++) socket.receive(new ArrayBuffer(1));
  expect(socket.close).toHaveBeenCalledOnce();
  expect(takePreparedDesktop(scope, 'agt/one')).toBeUndefined();
  expect(socket.send).not.toHaveBeenCalled();
  expect(vi.getTimerCount()).toBe(0);
});

it('does not open a connection in a hidden tab', () => {
  vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
  expect(prepareDesktop({}, 'agt/one', 'alice')).toBeUndefined();
  expect(DesktopSocket.instances).toHaveLength(0);
});
