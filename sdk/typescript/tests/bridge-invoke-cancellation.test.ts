import { createServer } from 'node:http';
import { once } from 'node:events';
import { expect, it } from 'vitest';
import { createClient, RequestAborted } from '../src/index.js';
import * as schema from '../src/bridges/invoke-schema.js';
import { loadCorpus } from './public-contract/context.js';

it('aborts observation after simulated commit and preserves exact request and key on explicit retry', async () => {
  const fixture = loadCorpus().find((item) => item.id === 'bridge-invoke-issue-explicit')!;
  const body = schema.issueRequest(fixture.request.body);
  const receipt = schema.receipt(fixture.response.body);
  const requests: { key: string | string[] | undefined; body: string }[] = [];
  let didCommit!: () => void;
  const committed = new Promise<void>((resolve) => {
    didCommit = resolve;
  });
  let release!: () => void;
  const released = new Promise<void>((resolve) => {
    release = resolve;
  });
  const server = createServer(async (request, response) => {
    expect(request.url).toBe(`/proxy/api/v1/bridges/${receipt.bridge_id}/invoke-grants`);
    expect(request.method).toBe('POST');
    expect(request.headers.authorization).toBe('Bearer fixture-manager');
    const chunks: Buffer[] = [];
    for await (const chunk of request) chunks.push(Buffer.from(chunk));
    requests.push({
      key: request.headers['idempotency-key'],
      body: Buffer.concat(chunks).toString(),
    });
    if (requests.length === 1) {
      didCommit();
      await released;
    }
    response.writeHead(201, { 'content-type': 'application/json' }).end(JSON.stringify(receipt));
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const address = server.address();
  if (address === null || typeof address === 'string') throw new Error('expected loopback address');
  const client = createClient({
    baseUrl: `http://127.0.0.1:${address.port}/proxy`,
    credential: () => 'fixture-manager',
  });
  const controller = new AbortController();
  try {
    const pending = client.bridges.issueInvokeGrant(receipt.bridge_id, body, {
      idempotencyKey: 'retain-command',
      signal: controller.signal,
    });
    const observed = pending.catch((error: unknown) => error);
    await committed;
    controller.abort();
    expect(await observed).toBeInstanceOf(RequestAborted);
    release();
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(requests).toHaveLength(1);
    expect(
      await client.bridges.issueInvokeGrant(receipt.bridge_id, body, {
        idempotencyKey: 'retain-command',
      }),
    ).toEqual(receipt);
    expect(requests).toHaveLength(2);
    expect(requests[0]).toEqual(requests[1]);
    expect(JSON.parse(requests[0]!.body)).toEqual(body);
    expect(requests[0]!.key).toBe('retain-command');
  } finally {
    release();
    server.closeAllConnections();
    await new Promise<void>((resolve, reject) =>
      server.close((error) => (error === undefined ? resolve() : reject(error))),
    );
  }
});
