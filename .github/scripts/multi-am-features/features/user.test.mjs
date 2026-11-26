import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {once} from 'node:events';
import {createServer} from 'node:http';
import test from 'node:test';
import {userCases} from './user.mjs';
import {NonOwnerRouter} from '../routing.mjs';

test('a file download preserves the real request ID for non-owner attribution', async () => {
  let directory, bytes;
  const proofs = [];
  const denied = [];
  const digest = data => createHash('sha256').update(data).digest('hex');
  const server = createServer((request, response) => {
    const url = new URL(request.url, 'http://loopback.test');
    assert.equal(url.pathname, '/api/v1/agents/fixture-agent/files/download');
    assert.equal(url.searchParams.get('path'), directory + '/message.txt');
    response.setHeader('x-request-id', 'fixture-file-download');
    response.end(bytes);
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const agents = {a: {ready: true, instance_id: 'instance-a', pod_uid: 'pod-a'},
    b: {ready: true, instance_id: 'instance-b', pod_uid: 'pod-b'}};
  const adapter = {
    ownerOf: async () => ({agent_id: 'fixture-agent', am: 'a', epoch: 1, ...agents.a}),
    topology: async () => ({shared_database: true, coordination_enabled: true, ams: agents}),
  };
  const api = {
    router: new NonOwnerRouter({adapter, record: proof => proofs.push(proof)}),
    headers: () => ({}),
    url: (am, route) => {
      assert.equal(am, 'b');
      return `http://127.0.0.1:${server.address().port}${route}`;
    },
    request: async (_am, method, route, options) => {
      const url = new URL(route, 'http://loopback.test');
      if (options?.body?.directory) {
        directory = options.body.path;
        return {body: {path: directory}};
      }
      if (method === 'POST') {
        if (options.allowed?.includes(409)) return {status: 409};
        bytes = Buffer.from(options.body.content_base64, 'base64');
        return {body: {sha256: digest(bytes)}};
      }
      if (method === 'DELETE') {
        directory = undefined;
        return {body: {deleted: true}};
      }
      if (url.pathname.endsWith('/files/tree')) {
        return {body: {entries: directory ? [{name: 'message.txt', kind: 'file'}] : []}};
      }
      return {body: {path: url.searchParams.get('path'), sha256: digest(bytes), content_base64: bytes.toString('base64')}};
    },
  };
  try {
    const outsider = {rawRequest: async (am, method, route, options) => {
      assert.deepEqual(options.allowed, [403, 404]);
      denied.push([am, method, new URL(route, 'http://loopback.test').pathname]);
      return {status: 404};
    }};
    await userCases({record: {agentId: 'fixture-agent'}, api, outsider,
      runCase: (name, execute) => name === 'user.files_roundtrip' ? execute() : undefined});
    assert.equal(proofs.length, 1);
    assert.equal(proofs[0].verdict, 'PASS');
    assert.equal(proofs[0].request_id, 'fixture-file-download');
    for (const am of ['a', 'b']) assert.deepEqual(denied.filter(row => row[0] === am).map(row => row[1]),
      ['GET', 'GET', 'POST', 'DELETE']);
    assert.equal(directory, undefined);
  } finally {
    server.closeAllConnections();
    await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  }
});
