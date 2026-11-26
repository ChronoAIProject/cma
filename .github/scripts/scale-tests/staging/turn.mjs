import { randomUUID } from 'node:crypto';
import { check, hash, safeFailure } from '../lib/checks.mjs';
import { TURN_MS, SETTLE_MS, FIRST_VISIBLE_MS } from './authority.mjs';
import { readTurn } from './transcript.mjs';

export class Turn {
  constructor(fixture, offer) {
    this.fixture = fixture; this.run = fixture.run; this.offer = offer;
    this.nonce = randomUUID(); this.answer = `OK ${this.nonce}`;
    this.prompt = `Only reply ${this.answer}. Do not call tools.`;
    this.key = `tl-scale-${hash(`${this.run.config.runId}:${fixture.spec.index}:turn:${offer}`)}`;
    this.state = 'not_issued'; this.settled = false;
    this.messages = []; this.starts = []; this.userCommands = [];
  }
  record(value) {
    return this.fixture.row({ scenario: 'rest_turn', offerHash: hash(this.offer), keyHash: hash(this.key),
      nonceHash: hash(this.nonce), ...value });
  }
  accept(frame) {
    if (frame.type === 'cma.agent.live') {
      if (frame.data.kind === 'text_delta' && frame.data.delta.length) {
        check(this.starts.length === 1 && frame.data.turn_sequence === this.starts[0].turn_sequence,
          'live_turn_binding_mismatch');
        this.firstDelta ??= this.run.time.now() - this.started;
      }
      return;
    }
    const event = frame.data; const kind = event.kind;
    if (kind?.type === 'message' && kind.role === 'user') {
      check(kind.content === this.prompt && typeof event.message_command_id === 'string', 'input_binding_missing_or_foreign');
      this.userCommands.push(event.message_command_id);
      check(this.userCommands.length === 1, 'duplicate_input');
    }
    if (kind?.type === 'turn_started') {
      check(event.response_binding?.turn_sequence === kind.turn_sequence, 'turn_command_binding_missing');
      this.starts.push(event.response_binding); check(this.starts.length === 1, 'overlapping_turn');
    }
    if (kind?.type === 'message' && kind.role === 'assistant') {
      this.messages.push(kind.content); check(this.messages.length === 1 && kind.content.trim() === this.answer,
        'assistant_answer_mismatch');
    }
    check(!kind?.type.startsWith('tool_') && kind?.type !== 'approval_requested', 'unexpected_tool_or_approval');
    if (['turn_failed', 'turn_cancelled'].includes(kind?.type)) check(false, 'turn_failed_or_cancelled');
    if (kind?.type === 'turn_completed') {
      check(this.starts.length === 1 && this.userCommands.length === 1
        && this.starts[0].command_id === this.userCommands[0]
        && this.starts[0].turn_sequence === kind.turn_sequence && this.messages.length === 1,
      'terminal_binding_mismatch');
      this.binding = this.starts[0]; this.streamTerminal = true;
      return;
    }
  }
  async durable(deadline) {
    const f = this.fixture;
    const { turn, evidenceHash } = await readTurn(f, this.prompt, deadline);
    if (!turn) return false;
    if (turn.outcome?.type === 'active') return false;
    check(turn.outcome?.type === 'completed' && Array.isArray(turn.agent_messages)
      && turn.agent_messages.length === 1 && turn.agent_messages[0].trim() === this.answer,
    'durable_terminal_or_answer_mismatch');
    if (this.binding) {
      check(turn.turn_sequence === this.binding.turn_sequence, 'durable_turn_mismatch');
      const { value: command } = await f.get(`/api/v2/commands/${encodeURIComponent(this.binding.command_id)}`, deadline);
      check(command.receipt?.command_id === this.binding.command_id && command.receipt.agent_id === f.agent
        && command.receipt.response_id === this.binding.response_id && command.turn_sequence === turn.turn_sequence,
      'command_receipt_mismatch');
      this.record({ status: 'terminal', commandHash: hash(this.binding.command_id), receiptHash: hash(JSON.stringify(command)) });
    }
    this.settled = true; this.record({ status: 'terminal', settled: true, evidenceHash });
    return true;
  }
  async execute() {
    const f = this.fixture;
    const setupDeadline = this.run.time.now() + 30000;
    await this.run.authority(); await f.http.session();
    check(f.primary?.ready && !f.primary.error, 'primary_observation_unavailable');
    await this.run.admit(f.spec, 'turn', setupDeadline);
    this.started = this.run.time.now(); const deadline = this.started + TURN_MS;
    this.state = 'uncertain';
    this.record({ status: 'issued', requestHash: hash(JSON.stringify({ text: this.prompt })) });
    const visible = f.spec.browserSample ? f.http.visible(f.agent, this.answer, FIRST_VISIBLE_MS, this.run.observation.signal).then(() => {
      this.visibleMs = this.run.time.now() - this.started;
    }).catch((error) => { this.visibleError = error; }) : null;
    try {
      const result = await f.http.request(`/api/v1/agents/${f.agent}/input`, { method: 'POST',
        body: JSON.stringify({ text: this.prompt }), key: this.key, expected: [202], timeoutMs: this.run.remaining(deadline) });
      check(result.value?.queued === true, 'invalid_admission_receipt');
      this.state = 'admitted'; this.ackMs = this.run.time.now() - this.started;
      this.record({ status: 'acknowledged', receiptHash: hash(JSON.stringify(result)), ackMs: this.ackMs });
      while (!this.streamTerminal) {
        if (this.streamError) throw this.streamError;
        if (this.visibleError) throw this.visibleError;
        await this.run.time.sleep(Math.min(50, this.run.remaining(deadline)));
      }
      check(await this.durable(deadline), 'terminal_not_durable');
      if (visible) await visible;
      if (this.visibleError) throw this.visibleError;
      this.run.remaining(deadline);
      const elapsedMs = this.run.time.now() - this.started;
      this.record({ status: 'PASS', ok: true, completed: 1, terminalMs: elapsedMs,
        ...(this.firstDelta === undefined ? {} : { streamFirstDeltaMs: this.firstDelta }),
        ...(this.visibleMs === undefined ? {} : { browserFirstVisibleMs: this.visibleMs }) });
      this.passed = true;
      return { ok: true, completed: true, evidenceHash: hash(`${this.key}:${this.binding.command_id}`) };
    } catch (error) {
      this.overdue = this.run.time.now() >= deadline;
      this.record({ ...safeFailure(error), overdue: this.overdue ? 1 : 0, ok: false });
      this.run.stop('turn_failure'); throw error;
    }
  }
  async settle(deadline = this.run.time.now() + SETTLE_MS) {
    if (this.state === 'not_issued') return;
    try {
      while (!this.settled) {
        if (await this.durable(deadline)) break;
        await this.run.time.sleep(Math.min(500, this.run.remaining(deadline)));
      }
    } catch (error) { this.record({ ...safeFailure(error), unresolved: 1, settled: false }); }
  }
}
