// A supervising runner supplies a private state directory and a generated token
// through the environment. Stdout is a sanitized JSON-lines control protocol.
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { createInterface } from 'node:readline';
import { startProviderFixture } from './provider-fixture.mjs';

let fixture, stopping = false;
const emit = row => process.stdout.write(JSON.stringify(row) + '\n');
const stop = async reason => {
  if (stopping) return;
  stopping = true;
  try {
    const receipt = await fixture?.close();
    emit({ type: 'closed', reason, receipt: receipt ?? null });
  } catch {
    emit({ type: 'error', code: 'fixture_cleanup_failed' });
    process.exitCode = 1;
  }
};
try {
  const args = process.argv.slice(2);
  if (args.length !== 2 || args[0] !== '--state-root') throw new Error('invalid_arguments');
  fixture = await startProviderFixture({ root: args[1], bearer: process.env.CMA_WORK_FIXTURE_BEARER });
  // All executable fixture dependencies are bound. No upstream binary/source
  // identity is inferred from these hashes or the readiness of this process.
  const sources = Object.fromEntries(['provider-cli.mjs', 'provider-fixture.mjs',
    'provider-ledger.mjs', 'provider-protocol.mjs'].map(name => [name,
    createHash('sha256').update(readFileSync(new URL(name, import.meta.url))).digest('hex')]));
  emit({ type: 'ready', schema: 'ordinary-work-provider-control-v1',
    origin: fixture.origin, identity: fixture.identity, sources,
    boundary: 'loopback synthetic provider only; no AM/runtime/admission/fault adapter' });
  const input = createInterface({ input: process.stdin, crlfDelay: Infinity });
  process.once('SIGTERM', () => { input.close(); void stop('signal'); });
  process.once('SIGINT', () => { input.close(); void stop('signal'); });
  for await (const line of input) {
    if (stopping) break;
    try {
      if (Buffer.byteLength(line) > 4096) throw new Error('command_too_large');
      const command = JSON.parse(line);
      if (!command || typeof command !== 'object' || Array.isArray(command)) throw new Error('invalid_command');
      const fields = { configure: ['command', 'modes'], release: ['action', 'command', 'request'],
        snapshot: ['command'], close: ['command'] };
      if (Object.keys(command).sort().join() !== fields[command.command]?.sort().join()) {
        throw new Error('invalid_command');
      }
      if (command.command === 'configure') fixture.configure(command.modes);
      else if (command.command === 'release') fixture.release(command.request, command.action);
      else if (command.command === 'close') { await stop('command'); input.close(); break; }
      emit({ type: 'snapshot', command: command.command, ledger: fixture.snapshot() });
    } catch {
      emit({ type: 'error', code: 'fixture_control_refused' });
      process.exitCode = 1;
      await stop('invalid_control'); input.close(); break;
    }
  }
  await stop('stdin_closed');
  process.stdin.destroy();
} catch {
  emit({ type: 'error', code: 'fixture_start_failed' });
  process.exitCode = 1;
  await stop('startup_failed');
}
