import { KNOWN_EVENTS } from './events';
import type { AguiTransport } from './connection';

/**
 * The real wire: the AG-UI interface's own replay route, for both the live
 * stream and the history behind it.
 *
 * It used to read `/api/v1/sessions/{id}/events`, which is REST's route and
 * answers in the core's vocabulary — `{"seq":…,"session_id":…,"event":{…}}`
 * with no top-level `type`. Every frame it delivered parsed as an unknown
 * event, so against production this surface could draw nothing else. The
 * `?stream=agui` it asked for was not a parameter that route has.
 *
 * `docs/API_AGUI.md` §7 is still the rule it follows: a dropped stream is
 * recovered by reading the log rather than by re-posting the run, which would
 * start a second turn. What changed is that the log is now readable in this
 * protocol's own words.
 *
 * **The sequence rides as the SSE id, not in the body.** The projection skips
 * it deliberately, so a third-party client is never handed a field that looks
 * like protocol and is not. That makes reading the id this transport's job:
 * everything above depends on `seq` for deduplication and for the cursor it
 * resumes on, and a frame that arrives without one silently breaks both.
 */

/** The session's stream, in AG-UI's vocabulary. */
function streamUrl(session: string): string {
  return `/api/v1/agui/events?thread_id=${encodeURIComponent(session)}`;
}

export const wireTransport: AguiTransport = {
  open(session, onFrame, onDrop) {
    const source = new EventSource(streamUrl(session), { withCredentials: true });

    const deliver = (message: MessageEvent) => {
      try {
        const frame = JSON.parse(String(message.data)) as Record<string, unknown>;
        // Put the sequence back where everything above expects to find it.
        // The server is right not to send it in the body; this is the seam
        // that is supposed to know the difference.
        onFrame({ ...frame, seq: Number(message.lastEventId) || 0 });
      } catch {
        // A frame that will not parse is not worth tearing the stream down
        // for. The durable log still holds it, and the next reconnect reads
        // it — which is the whole point of resuming on a cursor.
      }
    };

    /*
     * Unnamed frames, which is what the service sends. `onmessage` fires only
     * for these — a frame carrying an `event:` name is delivered to a listener
     * registered for that exact name and to nothing else, which is why this
     * handler alone received nothing at all when the service was naming them.
     */
    source.onmessage = deliver;

    /*
     * And named ones, for as long as any control plane still sends them.
     * A rolling deploy runs both builds at once and §5 of `CLAUDE.md` does not
     * allow that to interrupt a session, so this client reads either shape.
     *
     * A frame has one name or none, so the two paths are mutually exclusive
     * and nothing arrives twice — which matters, because a liveness frame
     * deliberately bypasses the deduplication upstream and would otherwise be
     * appended to the message being typed a second time.
     *
     * This is bounded by what this build knows, and that is the whole reason
     * the service stopped naming them: an `EventSource` cannot listen for a
     * name it has not heard of, so a kind added by a newer control plane can
     * only reach a client through the unnamed path above.
     */
    for (const kind of KNOWN_EVENTS) {
      source.addEventListener(kind, deliver as EventListener);
    }

    source.onerror = () => {
      onDrop('the event stream closed');
    };

    return () => {
      source.close();
    };
  },

  async history(session, cursor) {
    /*
     * The same route, read once rather than followed. `EventSource` cannot be
     * asked for a bounded read, so history is fetched over plain HTTP and
     * parsed out of the event-stream body — which is the price of the two
     * having one projection, and worth it: a turn read back and a turn watched
     * arriving must produce the same result (§18.1), and one code path is how
     * that stays true.
     */
    const url = `${streamUrl(session)}&starting_after=${String(cursor)}`;
    const response = await fetch(url, {
      credentials: 'same-origin',
      headers: { accept: 'text/event-stream' },
    });
    if (!response.ok) {
      throw new Error(`cannot read history: ${String(response.status)}`);
    }
    return parseEventStream(await response.text());
  },
};

/**
 * Read whole events out of an SSE body.
 *
 * Only the two fields that matter here: `id`, which carries the sequence, and
 * `data`, which carries the frame. A record without both is skipped rather
 * than guessed at.
 */
export function parseEventStream(body: string): unknown[] {
  const frames: unknown[] = [];
  for (const record of body.split('\n\n')) {
    let id: string | undefined;
    let data: string | undefined;
    for (const line of record.split('\n')) {
      if (line.startsWith('id:')) {
        id = line.slice(3).trim();
      } else if (line.startsWith('data:')) {
        data = line.slice(5).trim();
      }
    }
    if (data === undefined) {
      continue;
    }
    try {
      const frame = JSON.parse(data) as Record<string, unknown>;
      frames.push({ ...frame, seq: Number(id) || 0 });
    } catch {
      // Same reasoning as the live path: an unreadable frame is left behind
      // rather than allowed to end the read.
    }
  }
  return frames;
}
