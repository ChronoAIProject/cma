const assert = require("node:assert/strict");
const { createHash } = require("node:crypto");
const { mkdirSync, readFileSync, writeFileSync } = require("node:fs");
const { basename, join } = require("node:path");
const { word, workbook, archive } = require("./native_archive_fixtures.cjs");

const agentId = `agt_${"a".repeat(32)}`;
const files = [
  ["report.csv", Buffer.from("name,count\nExample,3\n"), "text/csv"],
  ["readme.md", Buffer.from("# Native notes\n\nFixed text.\n"), "application/octet-stream"],
  ["report.docx", word, "application/vnd.openxmlformats-officedocument.wordprocessingml.document"],
  ["report.xlsx", workbook, "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"],
  ["report.zip", archive, "application/zip"],
  ["report.json", Buffer.from('{"name":"Example","count":3}\n'), "application/json"],
].map(([name, bytes, mediaType]) => ({ path: `output/nested/${name}`, bytes, mediaType }));

function stageFiles(directory) {
  const source = join(directory, "fixture-source");
  mkdirSync(source);
  for (const file of files) writeFileSync(join(source, basename(file.path)), file.bytes);
}

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

module.exports = { files, stageFiles, digest, relayTool, requestText, verifyFileRead };
