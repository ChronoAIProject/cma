import WebSocket from 'ws';
import {performance} from 'node:perf_hooks';
import {check} from './core.mjs';

class Bytes {
  constructor(ws) {
    this.ws = ws; this.buffer = Buffer.alloc(0); this.waiter = null; this.error = null;
    ws.on('message', data => { this.buffer = Buffer.concat([this.buffer, Buffer.from(data)]); if (this.buffer.length > 4 * 1024 * 1024) this.error = new Error('rfb_byte_limit'); this.wake(); });
    ws.on('error', () => {this.error = new Error('desktop_socket_error'); this.wake();});
    ws.on('close', () => {this.error = new Error('desktop_socket_closed'); this.wake();});
  }
  wake() { this.waiter?.(); }
  async read(n) {
    const end = performance.now() + 15000;
    while (this.buffer.length < n) {
      if (this.error) throw this.error;
      check(performance.now() < end, 'rfb_read_timeout');
      await new Promise(resolve => { const timer = setTimeout(resolve, 100); this.waiter = () => {clearTimeout(timer); resolve();}; });
    }
    const value = this.buffer.subarray(0, n); this.buffer = this.buffer.subarray(n); return value;
  }
}
export async function desktopFrame(api, am, agentId) {
  const started = performance.now();
  const discovery = await api.rawRequest(am, 'GET', `/api/v1/agents/${agentId}/desktop`);
  const path = discovery.body?.websocket_url;
  check(path === `/api/v1/agents/${agentId}/desktop/ws`, 'desktop_route_identity_mismatch');
  const url = api.url(am, path); url.protocol = 'ws:';
  const ws = new WebSocket(url, {headers: api.headers(), handshakeTimeout: 15000, maxPayload: 4 * 1024 * 1024});
  let timing;
  ws.once('upgrade', response => {
    timing = {am, method: 'GET', route: '/api/v1/agents/{id}/desktop/ws',
      status: response.statusCode, header_ms: performance.now() - started,
      request_id: response.headers['x-request-id'] ?? null};
  });
  const bytes = new Bytes(ws);
  try {
    await new Promise((resolve, reject) => {ws.once('open', resolve); ws.once('error', () => reject(new Error('desktop_handshake_failed')));});
    const version = (await bytes.read(12)).toString('ascii');
    check(/^RFB 003\.00[378]\n$/.test(version), 'rfb_version_unsupported');
    const modern = !version.includes('003.003'); ws.send(Buffer.from(modern ? 'RFB 003.008\n' : version));
    if (modern) {
      const count = (await bytes.read(1))[0]; check(count > 0, 'rfb_security_refused');
      const types = await bytes.read(count); check(types.includes(1), 'rfb_security_unsupported'); ws.send(Buffer.from([1]));
      check((await bytes.read(4)).readUInt32BE(0) === 0, 'rfb_security_failed');
    } else { check((await bytes.read(4)).readUInt32BE(0) === 1, 'rfb_security_unsupported'); }
    ws.send(Buffer.from([1]));
    const init = await bytes.read(24); const width = init.readUInt16BE(0); const height = init.readUInt16BE(2); const nameLength = init.readUInt32BE(20);
    check(width > 0 && height > 0 && nameLength < 65536, 'rfb_server_init_invalid'); await bytes.read(nameLength);
    const pixel = Buffer.alloc(20); pixel[0] = 0; pixel[4] = 32; pixel[5] = 24; pixel[7] = 1;
    pixel.writeUInt16BE(255, 8); pixel.writeUInt16BE(255, 10); pixel.writeUInt16BE(255, 12); pixel[14] = 16; pixel[15] = 8; ws.send(pixel);
    const enc = Buffer.alloc(8); enc[0] = 2; enc.writeUInt16BE(1, 2); enc.writeInt32BE(0, 4); ws.send(enc);
    const request = Buffer.alloc(10); request[0] = 3; request.writeUInt16BE(1, 6); request.writeUInt16BE(1, 8); ws.send(request);
    const frame = await bytes.read(4); check(frame[0] === 0 && frame.readUInt16BE(2) > 0, 'rfb_frame_missing');
    let pixels = 0;
    for (let i = 0; i < frame.readUInt16BE(2); ++i) {
      const rect = await bytes.read(12); const rw = rect.readUInt16BE(4), rh = rect.readUInt16BE(6);
      check(rect.readInt32BE(8) === 0 && rw > 0 && rh > 0 && rw * rh <= width * height, 'rfb_raw_rectangle_invalid');
      await bytes.read(rw * rh * 4); pixels += rw * rh;
    }
    return {socket: ws, status: timing?.status, timing,
      body: {desktop_frame_received: true},
      result: {am, protocol: 'RFB', width, height, pixels_received: pixels, actual_frame_ms: performance.now() - started}};
  } catch (error) { ws.terminate(); throw error; }
}
export async function closeDesktop(ws) {
  if (ws.readyState === WebSocket.CLOSED) return;
  await new Promise(resolve => {const timer = setTimeout(() => {ws.terminate(); resolve();}, 3000); ws.once('close', () => {clearTimeout(timer); resolve();}); ws.close();});
}
