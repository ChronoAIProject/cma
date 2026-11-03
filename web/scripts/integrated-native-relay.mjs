import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { createServer } from 'node:http';
import { readFile, readdir, rename, unlink, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { WebSocketServer } from 'ws';

const directory = process.env.CMA_DESKTOP_RELAY_DIRECTORY;
assert(directory);
const atomic = async (name, value) => {
  await writeFile(join(directory, name + '.tmp'), JSON.stringify(value));
  await rename(join(directory, name + '.tmp'), join(directory, name));
};
const rpc = async (body) => {
  const identifier = randomUUID();
  await atomic(identifier + '.request.json', body);
  for (let attempt = 0; attempt < 3000; attempt += 1) {
    const bytes = await readFile(join(directory, identifier + '.response.json')).catch(
      () => undefined,
    );
    if (bytes) {
      await unlink(join(directory, identifier + '.response.json'));
      const response = JSON.parse(bytes);
      assert.equal(response.error, undefined, response.error);
      return response;
    }
    await delay(20);
  }
  throw new Error('Private runtime relay deadline');
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
      await rpc({ kind: 'provision', body: JSON.parse(body) });
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
  const identifier = randomUUID();
  let sent = rpc({ kind: 'socket_open', id: identifier, path: request.url });
  socket.on('message', (bytes, binary) => {
    sent = sent.then(() =>
      rpc({ kind: 'socket_send', id: identifier, binary, data: bytes.toString('base64') }),
    );
    sent.catch((error) => {
      console.error(error);
      socket.close();
    });
  });
  socket.on('close', () =>
    sent.then(() => rpc({ kind: 'socket_close', id: identifier })).catch(console.error),
  );
  (async () => {
    await sent;
    while (socket.readyState === 1) {
      const files = (await readdir(directory))
        .filter((name) => name.startsWith(identifier + '.') && name.endsWith('.packet.json'))
        .sort();
      for (const file of files) {
        const value = JSON.parse(await readFile(join(directory, file), 'utf8'));
        await unlink(join(directory, file));
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
await atomic('host-ready.json', { origin });
console.log('Exact runtime/private relay ready');
while (true) {
  for (const file of (await readdir(directory)).filter((name) =>
    /^public-[a-f0-9-]+\.request\.json$/.test(name),
  )) {
    const request = JSON.parse(await readFile(join(directory, file), 'utf8'));
    const ready = JSON.parse(await readFile(join(directory, 'public-ready.json'), 'utf8'));
    const agent = JSON.parse(await readFile(join(directory, 'agent.json'), 'utf8'));
    assert.equal(request.method, 'POST');
    assert.equal(request.path, '/api/v1/agents/' + agent.id + '/deliverable-references');
    const result = await fetch(ready.origin + request.path, {
      method: request.method,
      headers: {
        authorization: 'Bearer local-sdk-conformance',
        'content-type': 'application/json',
      },
      body: JSON.stringify(request.body),
    });
    await atomic(file.replace('.request.json', '.response.json'), {
      status: result.status,
      body: await result.json(),
    });
    await unlink(join(directory, file));
  }
  await delay(20);
}
