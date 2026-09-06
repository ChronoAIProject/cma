import { describe, expect, it } from 'vitest';
import { ApiError, ProtocolError, RequestAborted, ValidationError } from '../src/index.js';
import {
  agent,
  agentSummary,
  command,
  now,
  reference,
  resources,
  transport,
  workspace,
} from './resource-fixtures.js';

describe('Agent resource commands', () => {
  it('lists, reads and creates an Agent with pinned profile and optional first message', async () => {
    const test = transport();
    test.answer({ items: [agentSummary], next: 'next&' });
    expect(
      (
        await test.client.agents.list({
          sandbox: 'sandbox 1',
          workspace: 'workspace&1',
          status: 'ready',
          cursor: 'cursor?',
          limit: 1,
        })
      ).items,
    ).toEqual([agentSummary]);
    const params = new URL(test.calls.at(-1)!.url).searchParams;
    expect(Object.fromEntries(params)).toEqual({
      sandbox: 'sandbox 1',
      workspace: 'workspace&1',
      status: 'ready',
      cursor: 'cursor?',
      limit: '1',
    });
    test.answer(agent);
    expect(await test.client.agents.read(agent.id)).toEqual(agent);
    const body = {
      agent_profile: reference,
      title: 'Review',
      first_message: 'Inspect the changes.',
    };
    test.answer(agent, 201);
    expect(await test.client.agents.create(workspace.id, body, command)).toEqual(agent);
    expect(test.calls.at(-1)?.url).toContain(`/workspaces/${workspace.id}/agents`);
    expect(JSON.parse(String(test.calls.at(-1)?.init.body))).toEqual(body);
  });

  it('sends every operation exactly once with an explicit command key', async () => {
    const test = transport();
    test.answer({ queued: true }, 202);
    expect(await test.client.agents.input(agent.id, { text: 'Review' }, command)).toEqual({
      queued: true,
    });
    test.answer(undefined, 202);
    await test.client.agents.steer(agent.id, { text: 'Focus on tests' }, command);
    await test.client.agents.interrupt(agent.id, command);
    await test.client.agents.answerApproval(
      agent.id,
      'request/with ?#',
      { decision: 'allow' },
      command,
    );
    expect(test.calls.at(-1)?.url).toContain('/approvals/request%2Fwith%20%3F%23');
    await test.client.agents.stop(agent.id, { reason: 'owner' }, command);
    await test.client.agents.delete(agent.id, command);
    expect(test.calls.map(({ init }) => init.method)).toEqual([
      'POST',
      'POST',
      'POST',
      'POST',
      'POST',
      'DELETE',
    ]);
    for (const call of test.calls) {
      expect(new Headers(call.init.headers).get('idempotency-key')).toBe(command.idempotencyKey);
      expect(new Headers(call.init.headers).get('x-request-id')).toBe(command.requestId);
      expect(call.init.credentials).toBe('include');
    }
    expect(test.fetch).toHaveBeenCalledTimes(6);
  });

  it('retains explicit delivery errors without generating new keys or retrying', async () => {
    const test = transport();
    for (const code of ['idempotency_in_flight', 'idempotency_recovery_required']) {
      test.answer({ status: 409, code, detail: 'The operation has not settled' }, 409);
      await expect(
        test.client.agents.input(agent.id, { text: 'Review' }, command),
      ).rejects.toMatchObject({ status: 409, code });
    }
    expect(test.fetch).toHaveBeenCalledTimes(2);
    expect(test.calls.map(({ init }) => new Headers(init.headers).get('idempotency-key'))).toEqual([
      command.idempotencyKey,
      command.idempotencyKey,
    ]);
  });

  it('rejects invalid command keys before sending and cancelled calls without fetching', async () => {
    const test = transport();
    for (const idempotencyKey of ['', 'two words', '\n', 'a'.repeat(256), '\u0080']) {
      expect(() => test.client.agents.interrupt(agent.id, { idempotencyKey })).toThrow(
        ValidationError,
      );
    }
    expect(() => test.client.agents.read('..')).toThrow(ValidationError);
    expect(() => test.client.agents.read('\ud800')).toThrow(ValidationError);
    const controller = new AbortController();
    controller.abort();
    await expect(
      test.client.agents.read(agent.id, { signal: controller.signal }),
    ).rejects.toBeInstanceOf(RequestAborted);
    expect(test.fetch).not.toHaveBeenCalled();
  });
});

describe('Agent durable projections', () => {
  it('refuses missing or malformed chronological transcript messages', async () => {
    const test = transport();
    const message = { sequence: 2, role: 'user', content: 'Review', appended_at: now };
    for (const messages of [
      undefined,
      [{ ...message, sequence: 0 }],
      [{ ...message, role: 'system' }],
      [{ ...message, appended_at: null }],
    ]) {
      test.answer({
        agent_id: agent.id,
        through_seq: 2,
        turns: [
          {
            turn_sequence: 1,
            started_at: now,
            owner_message: 'Review',
            agent_messages: [],
            messages,
            outcome: { type: 'active' },
          },
        ],
      });
      await expect(test.client.agents.transcript(agent.id)).rejects.toBeInstanceOf(ProtocolError);
    }
  });

  it('decodes typed event pages, transcript outcomes, usage and nullable resource measurements', async () => {
    const test = transport();
    const events = {
      items: [{ seq: 1, appended_at: now, kind: { type: 'turn_started', turn_sequence: 1 } }],
      next: 1,
    };
    test.answer(events);
    expect(await test.client.agents.events(agent.id, { after: 0, limit: 1 })).toEqual(events);
    expect(test.calls.at(-1)?.url).toContain('/events?after=0&limit=1');
    const outcomes = [
      { type: 'active' },
      { type: 'completed', at: now },
      { type: 'failed', at: now, failure: { class: 'runtime', detail: 'Unavailable' } },
      { type: 'interrupted', at: now },
    ];
    const transcript = {
      agent_id: agent.id,
      through_seq: 8,
      turns: outcomes.map((outcome, index) => ({
        turn_sequence: index + 1,
        started_at: now,
        owner_message: 'Review',
        agent_messages: ['Reviewed'],
        messages: [
          { sequence: 2, role: 'user', content: 'Review', appended_at: now },
          { sequence: 3, role: 'assistant', content: 'Reviewed', appended_at: now },
          { sequence: 5, role: 'user', content: 'Check the boundary', appended_at: now },
        ],
        outcome,
      })),
    };
    test.answer(transcript);
    expect(await test.client.agents.transcript(agent.id)).toEqual(transcript);
    const usage = {
      agent_id: agent.id,
      through_seq: 8,
      input_tokens: 10,
      output_tokens: 20,
      context_used: 30,
    };
    test.answer(usage);
    expect(await test.client.agents.usage(agent.id)).toEqual(usage);
    test.answer(resources);
    expect(await test.client.agents.resources(agent.id)).toEqual(resources);
    test.answer({ ...resources, cpu_usage_usec: 0, memory_bytes_used: 1024 });
    expect(await test.client.agents.resources(agent.id)).toMatchObject({
      cpu_usage_usec: 0,
      memory_bytes_used: 1024,
    });
  });

  it('rejects wrong identities, unsafe counts, malformed events and unexpected response bodies', async () => {
    const test = transport();
    test.answer(resources);
    await expect(test.client.agents.resources('other')).rejects.toBeInstanceOf(ProtocolError);
    test.answer({ ...resources, uptime_ms: Number.MAX_SAFE_INTEGER + 1 });
    await expect(test.client.agents.resources(agent.id)).rejects.toBeInstanceOf(ProtocolError);
    test.answer({ items: [{ seq: 1, appended_at: now, kind: { type: 'invented_event' } }] });
    await expect(test.client.agents.events(agent.id)).rejects.toBeInstanceOf(ProtocolError);
    test.answer({ queued: 'true' }, 202);
    await expect(
      test.client.agents.input(agent.id, { text: 'Review' }, command),
    ).rejects.toBeInstanceOf(ProtocolError);
    test.answer({}, 202);
    await expect(test.client.agents.interrupt(agent.id, command)).rejects.toBeInstanceOf(
      ProtocolError,
    );
    test.answer(undefined, 200);
    await expect(test.client.agents.delete(agent.id, command)).rejects.toThrow(/expected HTTP 202/);
    test.answer({ status: 404, code: 'not_found', detail: 'Agent not found' }, 404);
    await expect(test.client.agents.read(agent.id)).rejects.toBeInstanceOf(ApiError);
  });
});
