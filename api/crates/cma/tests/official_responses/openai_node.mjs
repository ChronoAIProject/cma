import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";

const baseURL = process.env.CMA_SDK_BASE_URL;
const endpoint = new URL(baseURL);
assert.equal(endpoint.protocol, "http:");
assert.ok(["127.0.0.1", "localhost", "[::1]"].includes(endpoint.hostname));
const { default: OpenAI } = await import(pathToFileURL(process.env.CMA_NODE_SDK_ENTRY).href);
const client = new OpenAI({ baseURL, apiKey: "local-sdk-conformance", maxRetries: 0, timeout: 30000 });

const conversation = await client.conversations.create({});
assert.match(conversation.id, /^agt_/);
assert.equal(conversation.object, "conversation");
for (const field of ["sandbox", "workspace", "agent_profile"]) {
  assert.equal(typeof conversation.metadata[field], "string");
}
assert.deepEqual(await client.conversations.retrieve(conversation.id), conversation);

const frames = [];
const raw = await client.responses.create({
  conversation: conversation.id,
  model: "deployment-default",
  input: [{ role: "user", content: [{ type: "input_text", text: "sdk-raw-with-tools" }] }],
  metadata: { proof: "node-raw" },
  stream: true,
});
for await (const event of raw) frames.push(event);
assert.equal(frames[0].type, "response.created");
assert.equal(frames.at(-1).type, "response.completed");
frames.forEach((frame, index) => assert.equal(frame.sequence_number, index));
assert.ok(frames.some((frame) => frame.type === "response.reasoning_summary_text.delta"));
assert.ok(frames.some((frame) => frame.type === "response.cma_chat.tool_activity"));
const first = frames.at(-1).response;
const firstRead = await client.responses.retrieve(first.id);
assert.deepEqual(firstRead.output, first.output);
assert.deepEqual(firstRead.metadata, { proof: "node-raw" });
assert.equal(firstRead.output_text, "Hello from CMA");
assert.ok(firstRead.output.every((item) => item.type === "message"));

const items = [];
for await (const item of client.conversations.items.list(conversation.id, { limit: 1 })) items.push(item);
assert.ok(items.some((item) => item.id === first.output[0].id));
assert.ok(items.some((item) => item.type === "cma_chat.tool_activity"));
assert.ok(items.every((item) => item.type !== "reasoning"));

const helper = client.responses.stream({
  conversation: conversation.id,
  model: "deployment-default",
  input: "sdk-helper-text",
  previous_response_id: first.id,
});
const helperFrames = [];
for await (const event of helper) helperFrames.push(event);
const final = await helper.finalResponse();
assert.equal(final.status, "completed");
assert.equal(final.previous_response_id, first.id);
assert.equal(final.output_text, "Hello from CMA");
assert.ok(helperFrames.some((frame) => frame.type === "response.reasoning_summary_text.delta"));
assert.ok(helperFrames.some((frame) => frame.type === "response.output_item.added" && frame.item.type === "reasoning"));
assert.ok(final.output.every((item) => item.type === "message"));
const helperRead = await client.responses.retrieve(final.id);
// The aggregate helper adds local parsing state that is not a wire field.
const wireOutput = final.output.map((item) => ({
  ...item,
  content: item.content.map(({ parsed: _parsed, ...part }) => part),
}));
assert.deepEqual(helperRead.output, wireOutput);
await assert.rejects(client.responses.create({
  conversation: conversation.id, input: "Must not fork", stream: true,
}), (error) => error.status === 409 && error.code === "previous_response_id_conflict");

console.log(JSON.stringify({ sdk: "openai-node", conversation: conversation.id,
  rawFrames: frames.length, helperFrames: helperFrames.length, items: items.length, status: "passed" }));
