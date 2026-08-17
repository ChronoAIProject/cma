import type { AguiTransport } from './connection';

/**
 * The real wire: server-sent events for the stream, REST for the history.
 *
 * `docs/API_AGUI.md` §7 is explicit that a dropped stream is recovered by
 * reading the durable log rather than by re-posting the run — which would
 * start a second turn — so history comes from the session's event route and
 * never from the run endpoint.
 */
export const wireTransport: AguiTransport = {
  open(session, onFrame, onDrop) {
    const source = new EventSource(
      `/api/v1/sessions/${encodeURIComponent(session)}/events?stream=agui`,
      { withCredentials: true },
    );
    source.onmessage = (message: MessageEvent) => {
      try {
        onFrame(JSON.parse(String(message.data)));
      } catch {
        // A frame that will not parse is not worth tearing the stream down
        // for. The durable log still holds it, and the next reconnect reads
        // it — which is the whole point of resuming on a cursor.
      }
    };
    source.onerror = () => {
      onDrop('the event stream closed');
    };
    return () => {
      source.close();
    };
  },

  async history(session, cursor) {
    const url = `/api/v1/sessions/${encodeURIComponent(session)}/events?starting_after=${String(cursor)}`;
    const response = await fetch(url, { credentials: 'same-origin' });
    if (!response.ok) {
      throw new Error(`cannot read history: ${String(response.status)}`);
    }
    const body: unknown = await response.json();
    const events = (body as { events?: unknown }).events ?? body;
    return Array.isArray(events) ? events : [];
  },
};
