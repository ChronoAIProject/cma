import { execFile } from 'node:child_process';
import { constants } from 'node:fs';
import { open } from 'node:fs/promises';
import { check, ObservationError, requireActive } from './cma-bridge-continuity-errors.mjs';

const maxBytes = 4 * 1024 * 1024;

// Wait for the actual child to exit after cancellation. Merely rejecting a
// promise would leave kubectl running beyond the observation deadline.
export function executeReadOnly(file, args, { signal, ...options } = {}) {
  requireActive(signal);
  return new Promise((resolve, reject) => {
    const child = execFile(file, args, { ...options, killSignal: 'SIGKILL' }, (error, stdout) => {
      signal?.removeEventListener('abort', abort);
      try {
        requireActive(signal);
        if (error) throw new ObservationError('runtime_read_failed');
        resolve({ stdout });
      } catch (failure) { reject(failure); }
    });
    const abort = () => child.kill('SIGKILL');
    signal?.addEventListener('abort', abort, { once: true });
    if (signal?.aborted) abort();
  });
}

async function savedToken(path) {
  const file = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const stat = await file.stat();
    check(stat.isFile() && stat.uid === process.getuid() && (stat.mode & 0o077) === 0 && stat.size <= 16_384, 'private_token_file_required');
    const token = (await file.readFile('utf8')).trim();
    check(token.length > 0 && !/\s/.test(token), 'invalid_saved_token');
    return token;
  } finally { await file.close(); }
}

async function jsonBody(response, signal) {
  check(response.body, 'invalid_public_json');
  const reader = response.body.getReader();
  const chunks = [];
  let size = 0;
  try {
    for (;;) {
      requireActive(signal);
      const { done, value } = await reader.read();
      requireActive(signal);
      if (done) break;
      size += value.byteLength;
      check(size <= maxBytes, 'public_response_too_large');
      chunks.push(Buffer.from(value));
    }
    try { return JSON.parse(Buffer.concat(chunks).toString('utf8')); }
    catch { throw new ObservationError('invalid_public_json'); }
  } finally {
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}

export async function transport(config, { fetchImpl = fetch, execute = executeReadOnly } = {}) {
  const token = await savedToken(config.tokenFile);
  const agentPaths = new Set(config.agentIds.map((id) => `/api/v1/agents/${id}`));
  return {
    get: async (path, authenticated, { signal } = {}) => {
      requireActive(signal);
      const [base, query] = path.split('?');
      const eventPath = path === `${base}?${query}` && base.endsWith('/events') && agentPaths.has(base.slice(0, -7)) && /^after=(0|[1-9][0-9]*)&limit=100$/.test(query);
      check((path === '/' && authenticated === false) || (authenticated === true && (agentPaths.has(path) || eventPath)), 'public_read_outside_scope');
      const requestSignal = signal ? AbortSignal.any([signal, AbortSignal.timeout(15_000)]) : AbortSignal.timeout(15_000);
      try {
        const response = await fetchImpl(new URL(path, config.origin), {
          method: 'GET', headers: { 'User-Agent': 'Mozilla/5.0', ...(authenticated ? { Authorization: `Bearer ${token}` } : {}) },
          redirect: 'error', signal: requestSignal,
        });
        requireActive(requestSignal);
        if (!response.ok) {
          await response.body?.cancel();
          throw new ObservationError(`public_read_failed_${response.status}`);
        }
        if (!authenticated) { await response.body?.cancel(); return null; }
        return await jsonBody(response, requestSignal);
      } catch (error) {
        requireActive(signal);
        if (requestSignal.aborted) throw new ObservationError('public_read_timeout');
        throw error;
      }
    },
    pods: async ({ signal } = {}) => {
      requireActive(signal);
      const { stdout } = await execute('kubectl', ['--context', config.context, '--request-timeout=20s', '-n', config.namespace, 'get', 'pods', '-l', `cma/control-plane=${config.deploymentId}`, '-o', 'json'], { signal, timeout: 30_000, maxBuffer: maxBytes });
      requireActive(signal);
      try { return JSON.parse(stdout); }
      catch { throw new ObservationError('invalid_runtime_json'); }
    },
  };
}
