import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { Cmaeg } from '../dist/index.js';

function loopback(value) {
  const url = new URL(value);
  assert.ok(url.protocol === 'http:' && url.hostname === '127.0.0.1');
  assert.ok(url.username === '' && url.password === '' && url.search === '' && url.hash === '');
  return url.origin;
}
const root = loopback(process.env.CMAEG_FIXTURE_ROOT);
const config = await fetch(`${root}/config`).then((response) => response.json());
const baseUrl = loopback(config.gateway);
const creator = new Cmaeg({ baseUrl, credential: () => config.creator });
const tools = new Cmaeg({ baseUrl, credential: () => config.agent_key });
const body = { ...config.create, record_id: randomUUID() };
const created = await creator.createChannel(body, { idempotencyKey: randomUUID() });
const id = created.channel_id;
let current = created;
try {
  const recovered = await creator.recoverChannel(body.record_id);
  assert.ok(recovered.channel_id === id);
  current = await creator.getChannel(id);
  const update = structuredClone(body);
  delete update.record_id;
  delete update.credentials;
  update.expected_version = current.version;
  update.name = 'SDK updated policy';
  current = await creator.updateChannel(id, update);
  assert.ok(current.definition.name === update.name);
  assert.ok((await creator.testChannel(id)).accepted_after_ms >= 0);
  const event = {
    specversion: '1.0',
    id: randomUUID(),
    source: 'urn:sdk:typescript',
    type: 'sdk.test',
    data: { text: 'é'.repeat(16 * 1024) },
  };
  const options = () => ({
    webhook: {
      secret: config.signing,
      id: randomUUID(),
      timestamp: String(Math.floor(Date.now() / 1000)),
    },
  });
  const accepted = await creator.postEvent(id, event, options());
  assert.ok(accepted.event_id === event.id);
  async function reference(eventId) {
    const deadline = Date.now() + 5000;
    while (Date.now() < deadline) {
      const found = await fetch(`${root}/event-ref/${eventId}`).then((response) => response.json());
      if (typeof found.event_ref === 'string') return found.event_ref;
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
    throw new Error('provider did not retain the admitted event');
  }
  const ref = await reference(event.id);
  assert.ok(Buffer.byteLength(ref) <= 4096);
  const context = await tools.readEventContext(ref);
  assert.ok(context.activity.kind.ce.data.text === event.data.text);
  const restarted = await fetch(`${root}/restart`, { method: 'POST' });
  assert.ok(restarted.ok && (await restarted.json()).replaced === true);
  assert.deepEqual(await tools.readEventContext(ref), context);
  assert.ok((await tools.sendTyping(ref)).outcome === 'unsupported');
  const streamEvent = { ...event, id: randomUUID() };
  const stream = creator.streamEvent(id, streamEvent, options());
  assert.ok(!(await stream.next()).done);
  const streamedRef = await reference(streamEvent.id);
  const receipt = await tools.replyToEvent(streamedRef, { text: 'SDK explicit reply' });
  assert.ok(receipt.delivery.outcome === 'confirmed');
  const frames = [];
  for await (const frame of stream) frames.push(frame);
  assert.ok(
    frames.some(
      (frame) =>
        frame.event === 'cmaeg.message' && frame.data.message?.text === 'SDK explicit reply',
    ),
  );
  assert.ok(frames.at(-1)?.event === 'cmaeg.completed');
} finally {
  current = await creator.getChannel(id);
  await creator.deleteChannel(id, current.version);
}
await assert.rejects(
  () => creator.getChannel(id),
  (error) => error.code === 'channel_not_found',
);
console.log(
  'CMAEG TypeScript SDK: create, recover, update, test, signed event, full context after process replacement, typing, streamed reply and delete passed',
);
