#!/usr/bin/env node
import { spawn, execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants, createReadStream, createWriteStream } from 'node:fs';
import { chmod, copyFile, readFile, writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { check, hash, safeFailure } from './lib/checks.mjs';
import { workloadFor } from './lib/provider-start-schema.mjs';
import { reduceProviderStart } from './lib/provider-start.mjs';
import { prepareEvidenceDirectory } from './lib/evidence-directory.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const git = (...args) => execFileSync('git', args, { cwd: root, encoding: 'utf8' }).trim();
const clean = () => check(git('status', '--porcelain', '--untracked-files=all') === '', 'clean_candidate_required', true);
const sha256File = async (path) => {
  const digest = createHash('sha256');
  for await (const chunk of createReadStream(path)) digest.update(chunk);
  return digest.digest('hex');
};
const save = (path, value) => writeFile(path, `${JSON.stringify(value, null, 2)}\n`, { flag: 'wx', mode: 0o600 });

async function command(executable, args, env, log) {
  return new Promise((done, reject) => {
    const child = spawn(executable, args, { cwd: resolve(root, 'api'), env, stdio: ['ignore', 'pipe', 'pipe'] });
    const output = createWriteStream(log, { flags: 'wx', mode: 0o600 });
    let stdout = '';
    child.stdout.on('data', (chunk) => { stdout += chunk; output.write(chunk); });
    child.stderr.on('data', (chunk) => output.write(chunk));
    child.on('error', (error) => { output.end(); reject(error); });
    output.on('error', reject);
    child.on('close', (code) => output.end(() => {
      if (code === 0) done(stdout);
      else reject(new Error('local qualification command failed; inspect private log'));
    }));
  });
}

async function run(mode, outputPath) {
  const workload = workloadFor(mode);
  check(outputPath && process.env.CARGO_TARGET_DIR, 'output_and_assigned_cargo_lane_required');
  clean();
  const source = git('rev-parse', 'HEAD');
  const output = await prepareEvidenceDirectory(root, outputPath);
  const env = { ...process.env, CARGO_INCREMENTAL: '0', CARGO_BUILD_JOBS: '4', RUST_MIN_STACK: '33554432' };
  const build = await command('nice', ['-n', '10', process.env.CARGO ?? 'cargo', 'test', '--locked', '-p', 'cmaeg',
    '--lib', '--no-run', '--message-format=json'], env, resolve(output, 'build.log'));
  const artifacts = build.split('\n').filter((line) => line.startsWith('{')).map((line) => JSON.parse(line));
  const executable = artifacts.find((a) => a.reason === 'compiler-artifact' && a.target?.name === 'cmaeg'
    && a.target.kind.includes('lib') && a.profile?.test && a.executable)?.executable;
  check(executable, 'qualification_executable_missing');
  clean(); check(git('rev-parse', 'HEAD') === source, 'candidate_changed_during_build');
  const executableHash = await sha256File(executable);
  const qualifiedExecutable = resolve(output, 'qualification-executable');
  await copyFile(executable, qualifiedExecutable, constants.COPYFILE_EXCL);
  await chmod(qualifiedExecutable, 0o700);
  check(await sha256File(qualifiedExecutable) === executableHash, 'executable_changed_during_capture');
  const started = new Date().toISOString();
  await command(qualifiedExecutable, ['--exact', 'turns::routed::provider_start_tests::qualification::provider_start_local_qualification',
    '--ignored', '--nocapture'], { ...env, CMAEG_START_MODE: mode, CMAEG_START_OUTPUT: resolve(output, 'raw.json') },
  resolve(output, 'qualification.log'));
  const ended = new Date().toISOString();
  clean();
  check(git('rev-parse', 'HEAD') === source && await sha256File(qualifiedExecutable) === executableHash,
    'candidate_changed_during_qualification');
  const payload = JSON.parse(await readFile(resolve(output, 'raw.json'), 'utf8'));
  const candidate = { source_sha: source, executable_sha256: executableHash, image: 'not_deployed_local_test_executable' };
  const evidence = { schema_version: 1, candidate, started_at: started, ended_at: ended, workload, payload,
    payload_sha256: hash(JSON.stringify(payload)), bindings: payload.cohorts.map((row) => ({ candidate,
      config_sha256: hash(JSON.stringify(row.config)), workload_sha256: hash(JSON.stringify(workload)), process: row.measurement.process })) };
  await save(resolve(output, 'evidence.json'), evidence);
  const report = reduceProviderStart(evidence);
  await save(resolve(output, 'report.json'), report);
  console.log(JSON.stringify({ status: report.status, cohorts: report.cohorts.map((c) => ({ cohort: c.cohort,
    status: c.status, counts: c.counts, lag_ms: c.lag_ms })), real_load_canary_acceptance: report.real_load_canary_acceptance }, null, 2));
  // Threshold failure is a valid negative-control result, but never a successful gate exit.
  process.exitCode = report.status === 'PASS' ? 0 : report.status === 'BLOCKED' ? 2 : 1;
}

try {
  const [action, arg, output, ...extra] = process.argv.slice(2);
  check(extra.length === 0, 'invalid_provider_start_arguments');
  if (action === 'run') await run(arg, output);
  else if (action === 'reduce') {
    check(arg && !output, 'invalid_provider_start_arguments');
    const report = reduceProviderStart(JSON.parse(await readFile(arg, 'utf8')));
    console.log(JSON.stringify(report, null, 2));
    process.exitCode = report.status === 'PASS' ? 0 : report.status === 'BLOCKED' ? 2 : 1;
  } else if (action === '--help') {
    console.log('Usage: node provider-start.mjs run <burst|sustained|smoke> <private-output-directory>\n'
      + '       node provider-start.mjs reduce <evidence.json>\n'
      + 'Requires a clean Git candidate and assigned CARGO_TARGET_DIR. Default-off; only explicit runs collect.\n'
      + 'Burst: 100 simultaneous offers per cohort with 12s observations (negative queue-pressure control).\n'
      + 'Sustained: 30 wall-clock minutes at 2 events/s per cohort. Smoke: 4s, always incomplete.\n'
      + 'Four independent synthetic cold/bound and media/no-media preparation cohorts run concurrently.\n'
      + 'Reports always leave real load/canary acceptance BLOCKED. Exit 0 local PASS, 1 FAIL, 2 BLOCKED.');
  } else check(false, 'invalid_provider_start_arguments');
} catch (error) {
  const failure = safeFailure(error);
  console.error(JSON.stringify(failure));
  process.exitCode = failure.status === 'BLOCKED' ? 2 : 1;
}
