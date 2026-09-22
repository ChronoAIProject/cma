import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";

const legacyURL = process.env.CMA_SDK_BASE_URL;
const endpoint = new URL(legacyURL);
assert.equal(endpoint.protocol, "http:");
assert.ok(["127.0.0.1", "localhost", "[::1]"].includes(endpoint.hostname));
const entry = pathToFileURL(process.env.CMA_NODE_SDK_ENTRY);
const pkg = JSON.parse(await readFile(new URL("./package.json", entry), "utf8"));
assert.equal(pkg.version, "7.10.0");
const { default: OpenAI } = await import(entry.href);
const options = { apiKey: "local-sdk-conformance", maxRetries: 0, timeout: 30000 };
const legacy = new OpenAI({ ...options, baseURL: legacyURL });
const conversation = await legacy.conversations.create({});
const baseURL = legacyURL.replace(/\/api\/v1\/?$/, "/api/v2");
assert.notEqual(baseURL, legacyURL);
const client = new OpenAI({ ...options, baseURL });
assert.equal((await client.conversations.retrieve(conversation.id)).id, conversation.id);

const input = { conversation: conversation.id, input: "sdk-raw-with-tools", metadata: { proof: "node-v2" }, stream: true };
const frames = [];
for await (const frame of await client.responses.create(input, { headers: { "Idempotency-Key": "node-v2-first" } })) frames.push(frame);
assert.equal(frames[0].type, "response.created");
assert.equal(frames[0].response.status, "queued");
assert.equal(frames.at(-1).type, "response.completed");
frames.forEach((frame, index) => assert.equal(frame.sequence_number, index));
assert.ok(frames.every((frame) => !frame.type.includes("cma_chat") && !frame.type.includes("reasoning")));
const first = frames.at(-1).response;
const activity = first.output.filter((item) => item.type === "mcp_call");
assert.equal(activity.length, 1);
assert.equal(activity[0].status, "completed");
assert.equal(activity[0].server_label, "cma");
assert.equal(activity[0].name, "Check proof");
assert.equal(activity[0].arguments, "");
assert.equal(activity[0].output, null);
assert.equal(activity[0].error, null);
const activityFrames = frames.filter((frame) => frame.item_id === activity[0].id || frame.item?.id === activity[0].id);
assert.deepEqual(activityFrames.map((frame) => frame.type), ["response.output_item.added", "response.mcp_call.in_progress", "response.mcp_call.completed", "response.output_item.done"]);
const firstMessage = first.output.find((item) => item.type === "message");
assert.equal(first.conversation.id, conversation.id);
assert.equal(first.previous_response_id, null);
const firstRead = await client.responses.retrieve(first.id);
assert.deepEqual(firstRead.output, first.output);
assert.equal(firstRead.output_text, "Hello from CMA");
assert.deepEqual(firstRead.metadata, { proof: "node-v2" });

const alias = new OpenAI({ ...options, baseURL: `${baseURL}/agents/${conversation.id}` });
const retryFrames = [];
for await (const frame of await alias.responses.create(input, { headers: { "Idempotency-Key": "node-v2-first" } })) retryFrames.push(frame);
assert.deepEqual(retryFrames, frames);

const stream = client.responses.stream({ conversation: conversation.id, input: "sdk-helper-text" }, { headers: { "Idempotency-Key": "node-v2-second" } });
const helperFrames = [];
for await (const frame of stream) helperFrames.push(frame);
const final = await stream.finalResponse();
assert.equal(final.status, "completed");
assert.equal(final.output_text, "Hello from CMA");
assert.equal(final.previous_response_id, null);
assert.notEqual(final.id, first.id);
assert.equal(final.conversation.id, conversation.id);
const secondRead = await client.responses.retrieve(final.id);
assert.equal(secondRead.output_text, final.output_text);
assert.equal(secondRead.output[0].id, final.output[0].id);
const items = [];
for await (const item of client.conversations.items.list(conversation.id, { limit: 1 })) items.push(item);
assert.equal(items.length, 4);
assert.ok(items.some((item) => item.id === firstMessage.id));
assert.ok(items.some((item) => item.id === final.output[0].id));
assert.ok(items.every((item) => item.type === "message"));
for (const [body, code] of [
  [{ conversation: conversation.id, previous_response_id: first.id, input: "invalid", stream: true }, "conversation_previous_response_conflict"],
  [{ conversation: conversation.id, input: "invalid", stream: false }, "stream_required"],
  [{ conversation: conversation.id, input: "invalid", stream: true, model: "wrong-profile-model" }, "model_mismatch"],
]) await assert.rejects(client.responses.create(body), (error) => error.status === 422 && error.code === code);
const replay = await fetch(`${baseURL}/responses/${first.id}/events?starting_after=1`, { headers: { authorization: "Bearer local-sdk-conformance" } });
assert.equal(replay.status, 200);
const replayFrames = (await replay.text()).split("\n").filter((line) => line.startsWith("data: ")).map((line) => JSON.parse(line.slice(6)));
assert.deepEqual(replayFrames, frames.slice(2));
console.log(JSON.stringify({ sdk: "openai-node", version: pkg.version, versioned: 2, rawFrames: frames.length, helperFrames: helperFrames.length, items: items.length, status: "passed" }));
