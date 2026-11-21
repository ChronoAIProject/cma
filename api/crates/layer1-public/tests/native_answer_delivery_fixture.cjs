const assert = require("node:assert/strict");
const { createHash } = require("node:crypto");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");

const agentId = `agt_${"a".repeat(32)}`;
const files = [
  { path: "output/nested/report.csv", bytes: Buffer.from("name,count\nExample,3\n") },
  { path: "output/nested/readme.md", bytes: Buffer.from("# Native notes\n\nFixed text.\n") },
];

function digest(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

async function requestText(request) {
  let body = "";
  for await (const chunk of request) {
    body += chunk;
    assert(body.length < 2 * 1024 * 1024);
  }
  return body;
}

async function verifyFileRead(origin, directory, file) {
  assert.deepEqual(readFileSync(join(directory, file.path)), file.bytes);
  const target = new URL(`/api/v1/agents/${agentId}/files/content`, origin);
  target.searchParams.set("path", file.path);
  const response = await fetch(target, { signal: AbortSignal.timeout(10000) });
  assert.equal(response.status, 200, `Authorized read failed for ${file.path}`);
  const read = await response.json();
  assert.equal(read.path, file.path);
  assert.equal(read.size_bytes, file.bytes.length);
  assert.equal(read.sha256, digest(file.bytes));
  assert.deepEqual(Buffer.from(read.content_base64, "base64"), file.bytes);
}

async function relayTool(origin, request, response, receipts) {
  const names = ["authorization", "accept", "content-type", "mcp-protocol-version", "mcp-session-id"];
  const headers = new Headers();
  for (const name of names) {
    if (typeof request.headers[name] === "string") headers.set(name, request.headers[name]);
  }
  const body = await requestText(request);
  const upstream = await fetch(origin + "/api/v1/agent-tools", {
    method: request.method,
    headers,
    body: body || undefined,
    signal: AbortSignal.timeout(10000),
  });
  const text = await upstream.text();
  const rpc = body && JSON.parse(body);
  if (rpc?.method === "tools/call" && rpc.params?.name === "describe_deliverable") {
    assert.equal(upstream.status, 200);
    const result = JSON.parse(text).result;
    assert.equal(result.isError, false, JSON.stringify(result.structuredContent?.error));
    const receipt = result.structuredContent;
    const path = rpc.params.arguments.path;
    const previous = receipts.get(path);
    if (previous) assert.deepEqual(receipt, previous, "A repeated verification must preserve the receipt");
    receipts.set(path, receipt);
  }
  for (const name of ["content-type", "mcp-session-id"]) {
    const value = upstream.headers.get(name);
    if (value) response.setHeader(name, value);
  }
  response.writeHead(upstream.status);
  response.end(text);
}

module.exports = { files, digest, relayTool, requestText, verifyFileRead };
