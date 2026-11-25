import { check, safeFailure } from '../lib/checks.mjs';

export class PrimaryObservation {
  constructor(fixture) { this.fixture = fixture; this.controller = new AbortController(); this.ready = false; }
  async start(deadline) {
    const f = this.fixture;
    const { value } = await f.get(`/api/v2/agents/${f.agent}/events/head`, deadline);
    const head = f.run.wire.v2AgentEventHead(value, f.agent);
    check(head.active_turn === null, 'agent_not_idle');
    let ready; let fail;
    const handshake = new Promise((resolve, reject) => { ready = resolve; fail = reject; });
    const timer = setTimeout(() => { fail(new Error('observation_handshake_deadline')); this.controller.abort(); }, f.run.remaining(deadline));
    this.done = (async () => {
      const path = `/api/v2/agents/${f.agent}/events?after_cursor=${encodeURIComponent(head.through_cursor)}&follow=true`;
      const response = await f.http.stream(path, this.controller.signal);
      for await (const frame of f.run.wire.agentFrames(response, this.controller.signal, f.agent, head.through_cursor)) {
        // The server always emits this initial catch-up boundary, before any new offers.
        if (!this.ready && frame.type === 'cma.agent.live_gap' && frame.data.reason === 'reconnected'
          && frame.data.frames === null && frame.data.turn_sequence === null) {
          this.ready = true; ready(); continue;
        }
        check(frame.type !== 'cma.agent.live_gap' && frame.type !== 'cma.agent.error', 'observation_gap_or_error');
        const kind = frame.type === 'cma.agent.event' ? frame.data.kind?.type : 'live';
        if (f.active?.state !== 'not_issued' && f.active) f.active.accept(frame);
        else check(!['message', 'turn_started', 'live', 'tool_started'].includes(kind), 'unoffered_agent_work');
      }
      check(this.controller.signal.aborted, 'stream_ended_without_shutdown');
    })().catch((error) => {
      fail(error);
      if (!this.controller.signal.aborted) {
        this.error = error;
        if (f.active) f.active.streamError = error;
        f.run.stop('observation_failed');
        try { f.row({ scenario: 'primary_observation', ...safeFailure(error) }); }
        catch { /* The run's failed journal already closes admission and preserves cleanup custody. */ }
      }
    });
    try { await handshake; } finally { clearTimeout(timer); }
  }
  async close() { this.controller.abort(); await this.done; }
}
