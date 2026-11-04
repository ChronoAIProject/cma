import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { expect, it } from 'vitest';
import { createClient, RequestAborted, type ResponseEvent } from '../src/index.js';

const baseUrl = process.env.CMA_RESPONSE_COMPAT_URL;
const evidence = process.env.CMA_RESPONSE_COMPAT_EVIDENCE;
const agent = 'agt_response_compat';
const input = 'Run printf response-compat-proof';

it.skipIf(!baseUrl)(
  'decodes actual legacy output, disconnects, retrieves, and replays the same command',
  async () => {
    const captures: string[] = [];
    const reads: Record<string, unknown> = {};
    const client = createClient({
      baseUrl: baseUrl!,
      fetch: async (url, options) => {
        const response = await fetch(url, options);
        if (!response.headers.get('content-type')?.startsWith('text/event-stream')) return response;
        const index = captures.push('') - 1;
        const decoder = new TextDecoder();
        return new Response(
          response.body!.pipeThrough(
            new TransformStream({
              transform(chunk: Uint8Array, controller) {
                captures[index] += decoder.decode(chunk, { stream: true });
                controller.enqueue(chunk);
              },
            }),
          ),
          { status: response.status, headers: response.headers },
        );
      },
    });
    const update = async (value: unknown) => {
      const response = await fetch(`${baseUrl}/__compat/update`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(value),
      });
      expect(response.status).toBe(204);
    };
    const controller = new AbortController();
    const observed: ResponseEvent[] = [];
    let responseId = '';
    try {
      const outcome = await client.responses
        .create(
          { conversation: agent, input, stream: true },
          {
            idempotencyKey: 'response-compat-key',
            signal: controller.signal,
            onEvent: async (event) => {
              observed.push(event);
              if (event.type === 'response.created') {
                responseId = event.response.id;
                await update({
                  sessionUpdate: 'tool_call',
                  toolCallId: 'command-proof',
                  title: 'printf response-compat-proof',
                  kind: 'execute',
                  status: 'in_progress',
                });
              } else if (event.type === 'response.cma_chat.tool_activity') {
                if (observed.filter((value) => value.type === event.type).length === 1) {
                  await update({
                    sessionUpdate: 'tool_call_update',
                    toolCallId: 'command-proof',
                    content: [
                      {
                        type: 'content',
                        content: { type: 'text', text: 'response-compat-proof\n' },
                      },
                    ],
                  });
                } else {
                  controller.abort();
                }
              }
            },
          },
        )
        .catch((error: unknown) => error);
      expect(outcome).toBeInstanceOf(RequestAborted);
      const activities = observed.filter(
        (event) => event.type === 'response.cma_chat.tool_activity',
      );
      expect(activities).toHaveLength(2);
      expect(activities[1]).toMatchObject({
        response_id: responseId,
        item: {
          id: activities[0]!.item.id,
          call_id: 'command-proof',
          target: 'printf response-compat-proof',
          kind: 'execute',
          status: 'in_progress',
        },
      });
      expect(captures[0]).toContain('response-compat-proof\\n');
      expect(captures[0]).not.toContain('response.completed');
      reads.inProgress = await client.responses.read(responseId);
      expect(reads.inProgress).toMatchObject({
        id: responseId,
        status: 'in_progress',
        output: [],
      });
      const items = await client.responses.listConversationItems(agent);
      expect(items.data.find((item) => item.type === 'cma_chat.tool_activity')).toEqual(
        activities[1]!.item,
      );
      const recorded = await fetch(`${baseUrl}/api/v1/conversations/${agent}/items`).then(
        (response) => response.json(),
      );
      reads.reportedItems = recorded;
      expect(recorded.data[0].output).toEqual({
        text: 'response-compat-proof\n',
        truncated: false,
      });
      await update({
        sessionUpdate: 'tool_call_update',
        toolCallId: 'command-proof',
        status: 'completed',
      });
      await update({
        sessionUpdate: 'agent_message_chunk',
        content: { type: 'text', text: 'Command observed' },
      });
      await update({ end_turn: true });
      const retrieved = await client.responses.read(responseId);
      reads.completed = retrieved;
      expect(retrieved.status).toBe('completed');
      const replay: ResponseEvent[] = [];
      const completed = await client.responses.create(
        { conversation: agent, input, stream: true },
        {
          idempotencyKey: 'response-compat-key',
          onEvent: (event) => {
            replay.push(event);
          },
        },
      );
      expect(completed).toEqual(retrieved);
      expect(replay.slice(0, 4)).toEqual(observed);
      expect(
        replay
          .filter((event) => event.type === 'response.cma_chat.tool_activity')
          .map((event) => event.item.status),
      ).toEqual(['in_progress', 'in_progress', 'completed']);
      expect(replay.map((event) => event.sequence_number)).toEqual(replay.map((_, index) => index));
      reads.dispatches = await fetch(`${baseUrl}/__compat/dispatches`).then((response) =>
        response.json(),
      );
      expect(reads.dispatches).toEqual({ dispatches: 1 });
      reads.completedItems = await fetch(`${baseUrl}/api/v1/conversations/${agent}/items`).then(
        (response) => response.json(),
      );
      const refusal = await fetch(
        `${baseUrl}/api/v1/responses/${responseId}?stream=true&starting_after=3`,
      );
      expect(refusal.status).toBe(422);
      reads.resumeRefusal = await refusal.json();
      expect(reads.resumeRefusal).toMatchObject({ code: 'responses_field_unsupported' });
    } finally {
      controller.abort();
      if (evidence) {
        mkdirSync(evidence, { recursive: true });
        captures.forEach((wire, index) =>
          writeFileSync(join(evidence, `http-stream-${index}.sse`), wire),
        );
        writeFileSync(join(evidence, 'decoded-events.json'), JSON.stringify(observed, null, 2));
        writeFileSync(join(evidence, 'public-reads.json'), JSON.stringify(reads, null, 2));
      }
      await fetch(`${baseUrl}/__compat/shutdown`, { method: 'POST' });
    }
  },
  15_000,
);
