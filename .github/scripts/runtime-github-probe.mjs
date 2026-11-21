import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { readFileSync, statSync } from 'node:fs';
import { createServer } from 'node:http';
import { promisify } from 'node:util';

const execute = promisify(execFile);

export async function githubProxyProbe(initialCredential, control) {
  assert.equal(process.env.CHRONO_SANDBOX_REACHES, 'chrono-llm-public,ornn-api');
  const credentials = [initialCredential, ...[2, 3].map((generation) => ({
    t: 'credential', operation_id: `ci-github-scope-${generation}`,
    value: randomBytes(32).toString('hex'), custody_generation: generation,
  }))];
  let active = initialCredential;
  const directory = `/var/lib/cma-runtime/${process.env.CHRONO_SANDBOX_ID}/gh`;
  const configuration = JSON.parse(readFileSync(`${directory}/config.yml`, 'utf8'));
  assert.deepEqual(configuration, { http_unix_socket: `${directory}/api.sock`, git_protocol: 'https' });
  assert.equal(statSync(directory).mode & 0o777, 0o700);
  assert.equal(statSync(configuration.http_unix_socket).mode & 0o777, 0o600);
  const seen = [];
  let upstreamFailure = false;
  const server = createServer((request, response) => {
    const chunks = [];
    request.on('data', (chunk) => chunks.push(chunk));
    request.on('end', () => {
      try {
        assert.equal(request.headers.authorization, `Bearer ${active.value}`);
        assert.ok(!request.headers['open-sandbox-api-key']);
        const path = request.url;
        if (path === '/api/v1/proxy/s/api-github/user') assert.equal(request.method, 'GET');
        else {
          assert.equal(path, '/api/v1/proxy/s/api-github/graphql');
          assert.equal(request.method, 'POST');
          assert.deepEqual(JSON.parse(Buffer.concat(chunks)), { query: 'query { viewer { login } }' });
        }
        seen.push([active.custody_generation, path]);
        const granted = request.headers.authorization === `Bearer ${credentials[1].value}`;
        response.writeHead(granted ? 200 : 403, { 'content-type': 'application/json' });
        response.end(JSON.stringify(granted
          ? { login: 'offline-viewer', data: { viewer: { login: 'offline-viewer' } } }
          : { message: 'service not granted' }));
      } catch {
        upstreamFailure = true;
        response.writeHead(500);
        response.end('offline proxy fixture refused the request');
      }
    });
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(9192, '127.0.0.1', resolve);
  });
  const options = {
    encoding: 'utf8', timeout: 10000,
    env: {
      PATH: '/usr/local/bin:/usr/bin:/bin', HOME: '/home/cma',
      GH_CONFIG_DIR: directory, GH_HOST: 'github.com',
      GH_TOKEN: 'local-proxy-no-credential-needed', GH_PROMPT_DISABLED: '1',
    },
  };
  try {
    const privateOutput = (value) => credentials.every((credential) => !String(value).includes(credential.value));
    for (const credential of credentials) {
      if (credential !== initialCredential) {
        assert.deepEqual(await control(credential), {
          t: 'credential', operation_id: credential.operation_id, installed: true,
        });
      }
      active = credential;
      for (const args of [
        ['api', 'user', '--jq', '.login'],
        ['api', 'graphql', '-f', 'query=query { viewer { login } }', '--jq', '.data.viewer.login'],
      ]) {
        if (credential.custody_generation === 2) {
          const result = await execute('/usr/local/bin/gh', args, options);
          assert.equal(result.stdout.trim(), 'offline-viewer');
          assert.ok(privateOutput(result.stdout) && privateOutput(result.stderr));
        } else {
          await assert.rejects(execute('/usr/local/bin/gh', args, options),
            (error) => String(error.stderr).includes('HTTP 403')
              && privateOutput(error.stdout) && privateOutput(error.stderr));
        }
      }
    }
    await assert.rejects(execute('/usr/local/bin/gh', ['api', 'https://untrusted.example/user'], options),
      (error) => String(error.stderr).includes('HTTP 403') && privateOutput(error.stdout) && privateOutput(error.stderr));
    assert.equal(upstreamFailure, false);
    assert.deepEqual(seen, [1, 2, 3].flatMap((generation) => ['user', 'graphql']
      .map((path) => [generation, `/api/v1/proxy/s/api-github/${path}`])));
    return active;
  } finally {
    server.closeAllConnections();
    await new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  }
}
