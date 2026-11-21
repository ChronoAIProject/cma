import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { createServer } from 'node:http';
import { readFile, readdir, rename, unlink, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { WebSocketServer } from 'ws';

async function main() {
  const root = process.env.CMA_DESKTOP_RELAY_DIRECTORY;
  assert(root);
  const rpc = async (body) => {
    const identifier = randomUUID();
    const path = join(root, identifier);
    await writeFile(path + '.tmp', JSON.stringify(body));
    await rename(path + '.tmp', path + '.request.json');
    for (let attempt = 0; attempt < 3000; attempt += 1) {
      const bytes = await readFile(path + '.response.json').catch(() => undefined);
      if (bytes) {
        await unlink(path + '.response.json');
        const response = JSON.parse(bytes.toString());
        assert.equal(response.error, undefined, response.error);
        return response;
      }
      await delay(20);
    }
    throw new Error('Private native relay deadline');
  };
  const server = createServer(async (request, response) => {
    try {
      if (request.url.startsWith('/v1/sandboxes/') && request.url.includes('/endpoints/9090')) {
        response
          .writeHead(200, { 'content-type': 'application/json' })
          .end(JSON.stringify({ endpoint: origin }));
        return;
      }
      const chunks = [];
      for await (const chunk of request) chunks.push(chunk);
      const body = Buffer.concat(chunks);
      if (request.url === '/proof/provision') {
        await rpc({ kind: 'provision', body: JSON.parse(body.toString()) });
        response.writeHead(200).end('{}');
        return;
      }
      const result = await rpc({
        kind: 'http',
        path: request.url,
        method: request.method,
        headers: { 'content-type': request.headers['content-type'] ?? 'application/json' },
        body: body.length ? body.toString('base64') : undefined,
      });
      response.writeHead(result.status, {
        'content-type': result.headers['content-type'] ?? 'application/json',
      });
      response.end(Buffer.from(result.body, 'base64'));
    } catch (error) {
      console.error(error);
      if (!response.headersSent) response.writeHead(503);
      response.end();
    }
  });
  const sockets = new WebSocketServer({ server });
  sockets.on('connection', (socket, request) => {
    const id = randomUUID();
    let sent = rpc({ kind: 'socket_open', id, path: request.url });
    socket.on('message', (bytes, binary) => {
      sent = sent.then(() =>
        rpc({ kind: 'socket_send', id, binary, data: bytes.toString('base64') }),
      );
      sent.catch((error) => {
        console.error(error);
        socket.close();
      });
    });
    socket.on('close', () => {
      sent.then(() => rpc({ kind: 'socket_close', id })).catch(console.error);
    });
    (async () => {
      await sent;
      while (socket.readyState === 1) {
        const files = (await readdir(root))
          .filter((name) => name.startsWith(id + '.') && name.endsWith('.packet.json'))
          .sort();
        for (const file of files) {
          const value = JSON.parse(await readFile(join(root, file), 'utf8'));
          await unlink(join(root, file));
          socket.send(Buffer.from(value.data, 'base64'), { binary: value.binary });
        }
        await delay(15);
      }
    })().catch((error) => {
      console.error(error);
      socket.close();
    });
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const origin = 'http://127.0.0.1:' + server.address().port;
  await writeFile(join(root, 'host-ready.json'), JSON.stringify({ origin }));
  console.log('Private exact HTTP/ACP relay ready');
}
main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
