import assert from 'node:assert/strict';
import { createClient } from '../dist/index.js';
const root = process.env.CMA_SDK_FIXTURE_ROOT;
const url = new URL(root);
assert.equal(url.hostname, '127.0.0.1');
assert.equal(url.protocol, 'http:');
assert.equal(url.username, '');
assert.equal(url.password, '');
assert.equal(url.search, '');
assert.equal(url.hash, '');
const client = createClient({ baseUrl: root, credential: async () => 'local-sdk-conformance' });
const conversation = await client.responses.createConversation();
let prior;
for (let turn = 0; turn < 2; turn++) {
  const body = { conversation: conversation.id, input: 'sdk-helper-text', stream: true };
  const events = [];
  let receipt;
  const options = {
    idempotencyKey: `cma-node-${turn}`,
    onReceipt: (value) => {
      receipt = value;
    },
    onEvent: (event) => {
      events.push(event);
    },
  };
  const completed = await client.responsesV2.create(body, options);
  assert.equal(completed.output[0].content[0].text, 'Hello from CMA');
  assert.equal(events.length, 9);
  assert.notEqual(receipt.responseId, prior);
  assert.deepEqual(await client.responsesV2.read(receipt.responseId), completed);
  const alias = [];
  await client.responsesV2.createForAgent(conversation.id, body, {
    ...options,
    onEvent: (event) => {
      alias.push(event);
    },
  });
  assert.deepEqual(alias, events);
  const replay = [];
  await client.responsesV2.replay(receipt.responseId, {
    startingAfter: 4,
    onEvent: (event) => {
      replay.push(event);
    },
  });
  assert.deepEqual(replay, events.slice(5));
  prior = receipt.responseId;
}
assert.equal((await client.responsesV2.listConversationItems(conversation.id)).data.length, 4);
console.log(
  'CMA TypeScript Responses v2: two turns, receipts, alias, retrieval, replay, four items passed',
);
