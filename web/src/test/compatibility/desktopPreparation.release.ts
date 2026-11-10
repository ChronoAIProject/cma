export interface DesktopPreparationScope {
  readonly context?: object;
  readonly owner?: string;
}

interface Preparation {
  readonly agentId: string;
  readonly owner: string;
  readonly socket: WebSocket;
  readonly frames: MessageEvent<ArrayBuffer>[];
  readonly stop: () => void;
  readonly discard: () => void;
  transferred: boolean;
}

const preparations = new WeakMap<object, Preparation>();
const MAX_BYTES = 64 * 1024;
const MAX_FRAMES = 16;
const EXPIRY_MS = 10_000;

export function desktopSocketUrl(agentId: string) {
  const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
  return `${scheme}://${location.host}/api/v1/agents/${encodeURIComponent(agentId)}/desktop/ws`;
}

/** Connect only for an explicitly admitted Machine creation; the endpoint authorizes its owner. */
export function prepareDesktop(context: object, agentId: string, owner: string) {
  preparations.get(context)?.discard();
  if (document.hidden) return undefined;
  let socket: WebSocket;
  try {
    socket = new WebSocket(desktopSocketUrl(agentId));
  } catch {
    return undefined;
  }
  socket.binaryType = 'arraybuffer';
  const frames: MessageEvent<ArrayBuffer>[] = [];
  let bytes = 0;
  const stop = () => {
    clearTimeout(expiry);
    document.removeEventListener('visibilitychange', discard);
    socket.onmessage = socket.onerror = socket.onclose = null;
  };
  const discard = () => {
    stop();
    frames.length = 0;
    if (preparations.get(context) === value) preparations.delete(context);
    socket.close();
  };
  const expiry = setTimeout(discard, EXPIRY_MS);
  const value: Preparation = { agentId, owner, socket, frames, stop, discard, transferred: false };
  preparations.set(context, value);
  document.addEventListener('visibilitychange', discard);
  socket.onerror = socket.onclose = discard;
  socket.onmessage = (event: MessageEvent) => {
    if (
      !(event.data instanceof ArrayBuffer) ||
      frames.length >= MAX_FRAMES ||
      bytes + event.data.byteLength > MAX_BYTES
    ) {
      discard();
      return;
    }
    bytes += event.data.byteLength;
    frames.push(event);
  };
  return {
    handoff() {
      if (preparations.get(context) === value) value.transferred = true;
    },
    release() {
      if (!value.transferred) discard();
    },
  };
}

/** Consume once, after the personal Agent view has passed its normal access/runtime gates. */
export function takePreparedDesktop(scope: DesktopPreparationScope | undefined, agentId: string) {
  const context = scope?.context;
  const value = context === undefined ? undefined : preparations.get(context);
  if (!value || !value.transferred) return undefined;
  if (
    value.agentId !== agentId ||
    value.owner !== scope?.owner ||
    document.hidden ||
    value.socket.readyState > WebSocket.OPEN
  ) {
    value.discard();
    return undefined;
  }
  preparations.delete(context!);
  value.stop();
  return {
    socket: value.socket,
    replay() {
      // noVNC attaches synchronously. Preserve greetings received before attachment,
      // once and in order, after the viewer has installed its listeners and input gate.
      for (const frame of value.frames.splice(0)) value.socket.onmessage?.call(value.socket, frame);
    },
  };
}
