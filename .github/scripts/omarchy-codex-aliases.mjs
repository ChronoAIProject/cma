// Codex 0.165.0 owns arg0-linux helpers only for the lifetime of its process.
// Source: openai/codex rust-v0.165.0, codex-rs/arg0/src/lib.rs.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { existsSync, readdirSync, readlinkSync } from 'node:fs';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

export async function codexAliases(agentHome, version, onStage) {
  assert.equal(version, 'codex-cli 0.165.0');
  const executable = '/usr/local/lib/node_modules/@openai/codex/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex';
  const aliasRoot = `${agentHome}/tmp/arg0-linux`;
  const child = spawn(executable, ['app-server'], {
    env: { ...process.env, HOME: agentHome, CODEX_HOME: agentHome },
    stdio: ['pipe', 'ignore', 'ignore'],
  });
  const exited = once(child, 'exit');
  exited.catch(() => {}); // The finally block joins the child and observes spawn errors.
  try {
    onStage('agent-home-helper-process');
    await once(child, 'spawn');
    let aliases = [];
    const deadline = performance.now() + 10000;
    do {
      onStage('agent-home-helper-process');
      assert(child.exitCode === null && child.signalCode === null, 'Codex app-server exited before alias inspection');
      onStage('agent-home-helper-layout');
      if (existsSync(aliasRoot)) aliases = readdirSync(aliasRoot, { recursive: true, withFileTypes: true })
        .filter(entry => entry.isSymbolicLink());
      if (aliases.length === 3) break;
      await delay(50);
    } while (performance.now() < deadline);
    assert.deepEqual(aliases.map(entry => entry.name).sort(), ['apply_patch', 'applypatch', 'codex-linux-sandbox']);
    onStage('agent-home-helper-targets');
    for (const entry of aliases) assert.equal(readlinkSync(join(entry.parentPath, entry.name)), executable);
  } finally {
    // Simulate an abrupt exit so the original checkpoint tests still prove
    // escaping helpers are refused and exact exclusions preserve retained data.
    child.kill('SIGKILL');
    await exited;
  }
  return aliasRoot;
}
