import { performance } from 'node:perf_hooks';
import { setTimeout as sleep } from 'node:timers/promises';
import { check, hash, safeFailure } from './checks.mjs';
import { fixturePath, turnDeadlineMs, turnFixture } from './lc01-plan.mjs';

const resourceId = /^[A-Za-z0-9_-]{1,128}$/;
const terminal = ['completed', 'incomplete', 'failed', 'cancelled'];
const byteLimit = 4 * 1024 * 1024;
const cursorPattern = /^ag1\.([a-f0-9]{64})\.(0|[1-9]\d*)$/;

async function readBody(response) {
  check(response.body, 'body_missing');
  let received = 0;
  const chunks = [];
  for await (const chunk of response.body) {
    received += chunk.byteLength;
    check(received <= byteLimit, 'body_limit');
    chunks.push(chunk);
  }
  return JSON.parse(Buffer.concat(chunks).toString('utf8'));
}

export async function consumeEvents(body, receive) {
  check(body, 'stream_body_missing');
  const decoder = new TextDecoder('utf-8', { fatal: true });
  let pending = '';
  const data = [];
  let eventId;
  let frameSize = 0;
  for await (const chunk of body) {
    pending += decoder.decode(chunk, { stream: true });
    let delimiter;
    while ((delimiter = pending.indexOf('\n')) !== -1) {
      const line = pending.slice(0, delimiter).replace(/\r$/, '');
      pending = pending.slice(delimiter + 1);
      frameSize += Buffer.byteLength(line) + 1;
      check(frameSize <= byteLimit, 'stream_frame_limit');
      if (line === '') {
        const payload = data.join('\n');
        if (payload && payload !== '[DONE]' && receive(JSON.parse(payload), eventId) === false) return;
        data.length = 0;
        eventId = undefined;
        frameSize = 0;
      } else if (line.startsWith('data:')) {
        data.push(line.slice(5).replace(/^ /, ''));
      } else if (line.startsWith('id:')) {
        eventId = line.slice(3).replace(/^ /, '');
      }
    }
    check(Buffer.byteLength(pending) <= byteLimit, 'stream_line_limit');
  }
  pending += decoder.decode();
  check(!pending && data.length === 0, 'stream_frame_incomplete');
}

function inputRequest(agent, protocol, input) {
  switch (protocol) {
    case 'rest': return [`/api/v1/agents/${agent}/input`, { text: input }, 202];
    case 'agui': return ['/api/v1/agui/run', {
      threadId: agent, messages: [{ role: 'user', content: input }],
    }, 200];
    case 'responses': return [`/api/v2/agents/${agent}/responses`, { input, stream: true }, 200];
    default: throw new Error('unsupported_protocol');
  }
}

function streamResponse(response) {
  return response.headers.get('content-type')?.split(';')[0].trim() === 'text/event-stream';
}

async function discard(response) {
  await response.body?.cancel();
}

export class CapacityClient {
  constructor(origin, fixture, { fetcher = fetch, now = () => performance.now(), wait = sleep } = {}) {
    this.origin = origin;
    this.fixture = fixture;
    this.fetch = fetcher;
    this.now = now;
    this.wait = wait;
  }

  get agentPath() {
    return `/api/v1/agents/${this.fixture.agent}`;
  }

  request(path, { method = 'GET', body, key, stream = false, signal } = {}) {
    return this.fetch(this.origin + (this.fixture.prefix ?? '') + path, {
      method, redirect: 'error',
      signal: signal ?? AbortSignal.timeout(15_000),
      headers: {
        Authorization: `Bearer ${this.fixture.bearer}`,
        Accept: stream ? 'text/event-stream' : 'application/json',
        ...(key ? { 'Idempotency-Key': key } : {}),
        ...(body ? { 'Content-Type': 'application/json' } : {}),
      },
      body: body ? JSON.stringify(body) : undefined,
    });
  }

  async json(path, signal) {
    const response = await this.request(path, { signal });
    if (response.status === 200) return readBody(response);
    await discard(response);
    check(false, `http_${response.status}`);
  }

  read() {
    return this.json(this.agentPath);
  }

  async fileSha(path, signal, limit) {
    const query = new URLSearchParams({ path, max_bytes: String(limit) });
    const result = await this.json(`${this.agentPath}/files/content?${query}`, signal);
    check(result.path === path && typeof result.content_base64 === 'string', 'file_identity_changed');
    const contents = Buffer.from(result.content_base64, 'base64');
    const sha = hash(contents);
    check(contents.length <= limit && contents.length === result.size_bytes && sha === result.sha256,
      'file_content_digest_invalid');
    return sha;
  }

  async preflight(fixtureSha) {
    const agent = await this.read();
    check(agent.id === this.fixture.agent && agent.status === 'ready'
      && agent.sandbox?.id === this.fixture.sandbox, 'fixture_agent_not_ready', true);
    const sandbox = await this.json(`/api/v1/sandboxes/${this.fixture.sandbox}`);
    check(sandbox.id === this.fixture.sandbox && sandbox.status === 'active',
      'fixture_sandbox_not_active', true);
    check(await this.fileSha(fixturePath, undefined, 4096) === fixtureSha, 'fixture_file_digest_mismatch', true);
    const head = await this.json(`${this.eventsPath}/head`);
    check(head.agent_id === this.fixture.agent && head.active_turn === null
      && cursorPattern.test(head.through_cursor), 'fixture_history_not_idle', true);
    return head.through_cursor;
  }

  get eventsPath() {
    return `/api/v2/agents/${this.fixture.agent}/events`;
  }

  offerRestBinding(event) {
    const kind = event.kind;
    if (kind?.type === 'turn_started') {
      this.turnBinding = event.response_binding;
      return;
    }
    if (['turn_completed', 'turn_failed', 'turn_cancelled'].includes(kind?.type)) {
      this.turnBinding = undefined;
    }
    const wait = this.restBinding;
    if (wait && kind?.type === 'message' && kind.role === 'user'
      && hash(kind.content) === wait.inputHash
      && resourceId.test(this.turnBinding?.command_id ?? '')
      && resourceId.test(this.turnBinding?.response_id ?? '')) {
      wait.resolve(this.turnBinding);
    }
  }

  async observe(initialCursor, signal, record) {
    let cursor = initialCursor;
    const [, incarnation, initial] = cursorPattern.exec(initialCursor);
    let sequence = BigInt(initial);
    while (!signal.aborted) {
      const attempt = this.now();
      const query = new URLSearchParams({ after_cursor: cursor });
      record('stream_attempt', { cursor: sequence.toString() });
      const connection = new AbortController();
      const timer = setTimeout(() => connection.abort(), 15_000);
      try {
        const response = await this.request(`${this.eventsPath}?${query}`, {
          stream: true, signal: AbortSignal.any([signal, connection.signal]),
        });
        clearTimeout(timer);
        if (response.status !== 200 || !streamResponse(response)) {
          await discard(response);
          check(false, response.status === 200 ? 'stream_content_type_invalid' : `http_${response.status}`);
        }
        record('stream_open', { cursor: sequence.toString(), headersMs: this.now() - attempt });
        await consumeEvents(response.body, (event, id) => {
          if (id === undefined) return;
          const next = cursorPattern.exec(id);
          check(next && next[1] === incarnation && event.incarnation === incarnation
            && event.agent_id === this.fixture.agent && event.cursor === id
            && BigInt(next[2]) > sequence, 'stream_cursor_invalid');
          cursor = id;
          sequence = BigInt(next[2]);
          this.offerRestBinding(event);
          record('stream_cursor', { cursor: sequence.toString(), eventHash: hash(JSON.stringify(event)) });
        });
        record('stream_close', { cursor: sequence.toString(), reason: signal.aborted ? 'scheduled' : 'end_of_stream' });
      } catch (error) {
        record('stream_close', { cursor: sequence.toString(), reason: signal.aborted ? 'scheduled' : safeFailure(error).code });
      } finally {
        clearTimeout(timer);
      }
      if (!signal.aborted) {
        try {
          await this.wait(1000, undefined, { signal });
        } catch (error) {
          if (!signal.aborted) throw error;
        }
      }
    }
  }

  async turn({ run, user, at, protocol, started, fixtureSha, record, signal: runSignal }) {
    const deadline = AbortSignal.timeout(Math.max(1, Math.ceil(turnDeadlineMs - (this.now() - started))));
    const signal = runSignal ? AbortSignal.any([deadline, runSignal]) : deadline;
    const fixture = turnFixture(run, user, at, this.fixture.delaySeconds, fixtureSha);
    const [path, body, acceptedCode] = inputRequest(this.fixture.agent, protocol, fixture.input);
    let binding;
    const wait = protocol === 'rest' && Promise.withResolvers();
    if (wait) this.restBinding = { inputHash: hash(fixture.input), resolve: wait.resolve };
    let response;
    try {
      response = await this.request(path, { method: 'POST', body, key: fixture.key, stream: protocol !== 'rest', signal });
      if (protocol === 'rest' && response.status === acceptedCode) {
        await discard(response);
        record('acknowledged', { http: response.status, headersMs: this.now() - started, keyHash: hash(fixture.key) });
        let abort;
        const cancelled = new Promise((_, reject) => {
          abort = () => reject(signal.reason);
          if (signal.aborted) abort();
          else signal.addEventListener('abort', abort, { once: true });
        });
        try {
          binding = await Promise.race([wait.promise, cancelled]);
        } finally {
          signal.removeEventListener('abort', abort);
        }
      }
    } finally {
      if (wait) this.restBinding = undefined;
    }
    const http = response.status;
    const headersMs = protocol === 'rest' && binding ? undefined : this.now() - started;
    const command = binding?.command_id ?? response.headers.get('x-cma-command-id');
    const responseId = binding?.response_id ?? response.headers.get('x-cma-response-id');
    if (resourceId.test(command ?? '') && resourceId.test(responseId ?? '')) {
      record('admitted', { http, headersMs, commandHash: hash(command), responseHash: hash(responseId),
        keyHash: hash(fixture.key) });
    } else {
      await discard(response);
      const explicit = [400, 401, 403, 404, 422, 429].includes(http);
      record(explicit ? 'refused' : 'unknown', { http, headersMs, keyHash: hash(fixture.key) });
      return explicit;
    }
    if (http !== acceptedCode || (protocol !== 'rest' && !streamResponse(response))) {
      await discard(response);
      record('input_observation_failed', { http });
    } else if (protocol !== 'rest') {
      await this.observeTurn(response, protocol, responseId, record);
    }
    const durable = await this.responseStatus(responseId, signal);
    record('terminal', { state: durable.status, responseHash: hash(responseId), elapsedMs: this.now() - started });
    if (durable.status === 'completed') {
      const actualHash = await this.fileSha(fixture.outputPath, signal, 128);
      record('file_verified', { ok: actualHash === fixture.expectedHash });
    }
    return true;
  }

  async observeTurn(response, protocol, responseId, record) {
    let seenTerminal = false;
    let lastSequence = -1;
    try {
      await consumeEvents(response.body, (event) => {
        if (protocol === 'agui') {
          check(event.threadId === this.fixture.agent, 'turn_stream_agent_changed');
          seenTerminal = ['RUN_FINISHED', 'RUN_ERROR'].includes(event.type);
        } else {
          check(Number.isSafeInteger(event.sequence_number) && event.sequence_number > lastSequence,
            'response_stream_sequence_invalid');
          lastSequence = event.sequence_number;
          if (event.response?.id || event.response_id) {
            check((event.response?.id ?? event.response_id) === responseId, 'response_stream_identity_changed');
          }
          seenTerminal = ['response.completed', 'response.failed', 'response.incomplete'].includes(event.type);
        }
        if (seenTerminal) {
          record('submit_stream_terminal', { event: event.type });
          return false;
        }
      });
      check(seenTerminal, 'submit_stream_terminal_missing');
    } catch (error) {
      record('input_observation_failed', { reason: safeFailure(error).code });
    }
  }

  async responseStatus(id, signal) {
    for (;;) {
      const value = await this.json(`/api/v2/responses/${id}`, signal);
      check(value.id === id && value.conversation?.id === this.fixture.agent, 'response_identity_changed');
      if (terminal.includes(value.status)) return value;
      check(['queued', 'in_progress'].includes(value.status), 'unexpected_response_state');
      await this.wait(2000, undefined, { signal });
    }
  }
}
