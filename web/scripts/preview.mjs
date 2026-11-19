import { spawn } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import { stripVTControlCharacters } from 'node:util';

export function localPath(value, origin) {
  try {
    const url = new URL(value);
    return url.origin === origin ? url.pathname.slice(0, 160) : '<outside preview>';
  } catch {
    return '<invalid URL>';
  }
}
function safeLog(text, directory) {
  return stripVTControlCharacters(text)
    .replaceAll(directory, '<workspace>')
    .replace(/https?:\/\/[^\s]+/g, (value) => {
      try {
        const url = new URL(value);
        return `${url.protocol}//${url.host}${url.pathname}`;
      } catch {
        return '<URL>';
      }
    })
    .replace(/(?:token|password|secret|authorization)\s*[:=]\s*\S+/gi, '<redacted>')
    .slice(-2048);
}

export async function startPreview({ port, directory = process.cwd() }) {
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error('Invalid preview port');
  const origin = `http://127.0.0.1:${port}`;
  const expected = JSON.parse(await readFile(join(directory, 'dist/build-info.json'), 'utf8'));
  const index = await readFile(join(directory, 'dist/index.html'), 'utf8');
  const server = spawn(
    process.execPath,
    [
      fileURLToPath(new URL('../node_modules/vite/bin/vite.js', import.meta.url)),
      'preview',
      '--host',
      '127.0.0.1',
      '--strictPort',
      '--port',
      String(port),
    ],
    { cwd: directory, stdio: ['ignore', 'pipe', 'pipe'] },
  );
  let output = '';
  let failure;
  let timeout;
  const ended = new Promise((resolve) => server.once('close', resolve));
  const close = async () => {
    clearTimeout(timeout);
    if (server.exitCode !== null || server.signalCode !== null) return;
    server.kill('SIGTERM');
    const force = setTimeout(() => server.kill('SIGKILL'), 2000);
    try {
      await ended;
    } finally {
      clearTimeout(force);
    }
  };
  const assertAlive = () => {
    if (failure) throw failure;
    if (server.exitCode !== null || server.signalCode !== null)
      throw new Error(`Owned preview exited on port ${port}: ${output}`);
  };
  try {
    await new Promise((resolve, reject) => {
      const fail = (reason) => {
        failure = new Error(`Owned preview ${reason} on port ${port}: ${output}`);
        reject(failure);
      };
      server.once('error', () => fail('could not start'));
      server.once('exit', (code) => fail(`exited (${code ?? 'signal'})`));
      const receive = (chunk) => {
        output = safeLog(output + String(chunk), directory);
        if (output.includes(`${origin}/`)) resolve();
      };
      server.stdout.on('data', receive);
      server.stderr.on('data', receive);
      timeout = setTimeout(() => fail('startup timed out'), 10_000);
    });
    clearTimeout(timeout);
    assertAlive();
    const identity = await fetch(`${origin}/build-info.json`, {
      signal: AbortSignal.timeout(4000),
    });
    const value = await identity.json();
    if (!identity.ok || JSON.stringify(value) !== JSON.stringify(expected))
      throw new Error('Owned preview build identity does not match dist/build-info.json');
    const served = await fetch(`${origin}/`, { signal: AbortSignal.timeout(4000) });
    if (!served.ok || (await served.text()) !== index)
      throw new Error('Owned preview document does not match dist/index.html');
    assertAlive();
    return { origin, close, assertAlive, diagnostics: () => output };
  } catch (error) {
    await close();
    throw error;
  }
}
