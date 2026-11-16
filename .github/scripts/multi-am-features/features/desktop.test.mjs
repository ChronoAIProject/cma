import test from 'node:test';
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {once} from 'node:events';
import {WebSocketServer} from 'ws';
import {desktopFrame, closeDesktop} from '../cma/desktop.mjs';
import {NonOwnerRouter} from '../routing.mjs';

// Actual local Upgrade and RFB framing; no AM, native desktop or product PASS.
test('desktop attribution uses the socket request and preserves frame receipt on missing correlation', async () => {
  for (const requestId of ['socket-request', null]) {
    const server = createServer(), sockets = new WebSocketServer({server});
    sockets.on('headers', headers => { if (requestId) headers.push('x-request-id: ' + requestId); });
    sockets.on('connection', ws => {
      const init = Buffer.alloc(24); init.writeUInt16BE(1, 0); init.writeUInt16BE(1, 2);
      const frame = Buffer.from([0, 0, 0, 1]), rectangle = Buffer.alloc(12);
      rectangle.writeUInt16BE(1, 4); rectangle.writeUInt16BE(1, 6);
      ws.send(Buffer.concat([Buffer.from('RFB 003.008\n'), Buffer.from([1, 1, 0, 0, 0, 0]),
        init, frame, rectangle, Buffer.alloc(4)]));
    });
    server.listen(0, '127.0.0.1'); await once(server, 'listening');
    const api = {headers: () => ({}), url: (_am, path) => new URL(path, 'http://127.0.0.1:' + server.address().port),
      rawRequest: async () => ({body: {websocket_url: '/api/v1/agents/agent/desktop/ws'},
        timing: {request_id: 'discovery-is-a-different-request'}})};
    let opened, calls = 0;
    const owner = {agent_id: 'agent', epoch: 1, am: 'a', instance_id: 'a-instance', pod_uid: 'a-pod'};
    const router = new NonOwnerRouter({record: async () => {}, adapter: {
      ownerOf: async () => owner, topology: async () => ({shared_database: true, coordination_enabled: true,
        ams: {a: {ready: true, instance_id: 'a-instance', pod_uid: 'a-pod'},
          b: {ready: true, instance_id: 'b-instance', pod_uid: 'b-pod'}}})}});
    try {
      const operation = () => router.dispatch('agent', async ingress => {
        calls++; opened = await desktopFrame(api, ingress, 'agent'); return opened;
      });
      if (requestId) {
        const result = await operation();
        assert.equal(result.timing.request_id, requestId);
        assert.equal(result.status, 101);
        assert.equal(result.result.pixels_received, 1);
      } else {
        await assert.rejects(operation(), error => {
          assert.equal(error.response.status, 101);
          assert.equal(error.response.body.desktop_frame_received, true);
          assert.equal('socket' in error.response, false);
          return error.message === 'response_request_id_required';
        });
      }
      assert.equal(calls, 1);
    } finally {
      if (opened) await closeDesktop(opened.socket);
      for (const ws of sockets.clients) ws.terminate();
      await new Promise(resolve => sockets.close(resolve));
      await new Promise(resolve => server.close(resolve));
    }
  }
});
