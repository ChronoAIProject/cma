import type { Dispatch } from './context.js';
import { TransportError } from '../../src/errors.js';

async function captureLegacyStream(prefix: boolean | undefined, run: () => Promise<unknown>) {
  try {
    await run();
  } catch (error) {
    if (prefix && error instanceof TransportError) return;
    throw error;
  }
  if (prefix) throw new Error('An open stream prefix must report an incomplete transport');
}

export const experienceDispatch: Record<string, Dispatch> = {
  'POST /api/v2/agents/{agent_id}/responses/{response_id}/interrupt': ({
    client,
    param,
    body,
    fixture,
  }) =>
    client.responsesV2.interruptWithInput(param('agent_id'), param('response_id'), body, {
      idempotencyKey:
        Object.entries(fixture.request.headers).find(
          ([name]) => name.toLowerCase() === 'idempotency-key',
        )?.[1] ?? '',
    }),
  'GET /api/v2/agents/{agent_id}/responses/{response_id}/controls': ({ client, param }) =>
    client.responsesV2.observeControls(param('agent_id'), param('response_id')),
  'POST /api/v2/agents/{agent_id}/responses/{response_id}/controls': ({
    client,
    param,
    body,
    fixture,
  }) =>
    client.responsesV2.submitControl(param('agent_id'), param('response_id'), body, {
      idempotencyKey:
        Object.entries(fixture.request.headers).find(
          ([name]) => name.toLowerCase() === 'idempotency-key',
        )?.[1] ?? '',
    }),
  'GET /api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation': ({
    client,
    param,
  }) => client.responsesV2.observeQueuedCancellation(param('agent_id'), param('response_id')),
  'POST /api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation': ({
    client,
    param,
    body,
    fixture,
  }) =>
    client.responsesV2.cancelQueuedResponse(param('agent_id'), param('response_id'), body, {
      idempotencyKey:
        Object.entries(fixture.request.headers).find(
          ([name]) => name.toLowerCase() === 'idempotency-key',
        )?.[1] ?? '',
    }),
  'GET /api/v2/agents/{agent_id}/queued-inputs': ({ client, param }) =>
    client.responsesV2.listQueuedInputs(param('agent_id')),
  'POST /api/v2/agents/{agent_id}/responses/{response_id}/queued-input': ({
    client,
    param,
    body,
    fixture,
  }) =>
    client.responsesV2.editQueuedInput(param('agent_id'), param('response_id'), body, {
      idempotencyKey:
        Object.entries(fixture.request.headers).find(
          ([name]) => name.toLowerCase() === 'idempotency-key',
        )?.[1] ?? '',
    }),
  'GET /api/v2/commands/{command_id}': ({ client, param }) =>
    client.responsesV2.readCommand(param('command_id')),
  'GET /api/v2': ({ client }) => client.responsesV2.readCapabilities(),
  'GET /api/v2/conversations/{conversation_id}': ({ client, param }) =>
    client.responsesV2.readConversation(param('conversation_id')),
  'GET /api/v2/conversations/{conversation_id}/items': ({ client, param, options }) =>
    client.responsesV2.listConversationItems(param('conversation_id'), options),
  'GET /api/v2/responses/{response_id}': ({ client, param }) =>
    client.responsesV2.read(param('response_id')),
  'GET /api/v2/responses/{response_id}/output': ({ client, param, query }) =>
    client.responsesV2.readOutputPage(param('response_id'), { after: query.get('after') ?? undefined }),
  'GET /api/v2/responses/{response_id}/events': async ({ client, param, query }) => {
    const events: unknown[] = [];
    await client.responsesV2.replay(param('response_id'), {
      startingAfter: query.has('starting_after') ? Number(query.get('starting_after')) : undefined,
      onEvent: (event) => {
        events.push(event);
      },
    });
    return events;
  },
  'POST /api/v2/responses': async ({ client, body, command }) => {
    const events: unknown[] = [];
    await client.responsesV2.create(body, {
      idempotencyKey: command.idempotencyKey,
      onEvent: (event) => {
        events.push(event);
      },
    });
    return events;
  },
  'POST /api/v2/agents/{agent_id}/responses': async ({ client, body, command, param }) => {
    const events: unknown[] = [];
    await client.responsesV2.createForAgent(param('agent_id'), body, {
      idempotencyKey: command.idempotencyKey,
      onEvent: (event) => {
        events.push(event);
      },
    });
    return events;
  },
  'GET /api/v1/agui/capabilities': ({ client }) => client.agui.readCapabilities(),
  'GET /api/v1/agui/agents/{id}/sandbox': ({ client, param }) =>
    client.agui.readSandboxSnapshot(param('id')),
  'GET /api/v1/agui/events': async ({ client, query }) => {
    const events: unknown[] = [];
    await client.agui.replay(query.get('thread_id')!, {
      after: query.has('starting_after') ? Number(query.get('starting_after')) : undefined,
      follow: query.get('follow') !== 'false',
      maxReconnects: 0,
      onEvent: (event) => {
        events.push(event);
      },
    });
    return events;
  },
  'POST /api/v1/agui/run': async ({ client, body, fixture }) => {
    const events: unknown[] = [];
    await captureLegacyStream(fixture.response.streamPrefix, () =>
      client.agui.run(body, {
        idempotencyKey: Object.entries(fixture.request.headers).find(
          ([name]) => name.toLowerCase() === 'idempotency-key',
        )?.[1],
        onEvent: (event) => {
          events.push(event);
        },
      }),
    );
    return events;
  },
  'GET /api/v1/conversations/{id}': ({ client, param }) =>
    client.responses.readConversation(param('id')),
  'POST /api/v1/conversations': ({ client, body }) => client.responses.createConversation(body),
  'GET /api/v1/conversations/{id}/items': ({ client, param, options }) =>
    client.responses.listConversationItems(param('id'), options),
  'GET /api/v1/responses/{response_id}': ({ client, param }) =>
    client.responses.read(param('response_id')),
  'DELETE /api/v1/responses/{response_id}': ({ client, param }) =>
    client.responses.delete(param('response_id')),
  'POST /api/v1/responses/{response_id}/cancel': ({ client, param }) =>
    client.responses.cancel(param('response_id')),
  'POST /api/v1/responses': async ({ client, body, fixture }) => {
    const events: unknown[] = [];
    await captureLegacyStream(fixture.response.streamPrefix, () =>
      client.responses.create(body, {
        idempotencyKey: Object.entries(fixture.request.headers).find(
          ([name]) => name.toLowerCase() === 'idempotency-key',
        )?.[1],
        onEvent: (event) => {
          events.push(event);
        },
      }),
    );
    return events;
  },
  'GET /api/v1/agui/launches/{id}/events': async ({ client, param, query }) => {
    const events: unknown[] = [];
    await client.launches.stream(param('id'), {
      after: query.has('starting_after') ? Number(query.get('starting_after')) : undefined,
      follow: query.has('follow')
        ? query.get('follow') === 'true'
          ? true
          : query.get('follow') === 'false'
            ? false
            : (query.get('follow') as never)
        : undefined,
      limit: query.has('limit') ? Number(query.get('limit')) : undefined,
      onEvent: (event) => {
        events.push(event);
      },
    });
    return events;
  },
};
