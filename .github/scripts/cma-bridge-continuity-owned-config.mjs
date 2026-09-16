// Shared configuration for the existing owned-canary continuation/browser tools.
import { readFile, mkdir, lstat, realpath } from 'node:fs/promises';
import { isAbsolute, relative, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { digest, validateConfig } from './cma-bridge-continuity-observer.mjs';

const check = (condition, code) => { if (!condition) throw new Error(code); };
const marker = (value) => typeof value === 'string' && /^[A-Za-z0-9-]{1,160}$/.test(value);
const identifier = (value, prefix) => typeof value === 'string' && new RegExp(`^${prefix}_[A-Za-z0-9_-]{1,196}$`).test(value);

export function validateOwnedConfig(config) {
  check(config?.schema === 'cma-owned-continuity-config-v1', 'owned_config_schema_required');
  check(typeof config.run === 'string' && /^[a-z0-9][a-z0-9-]{0,63}$/.test(config.run), 'bounded_run_identity_required');
  check(typeof config.expectedSubject === 'string' && config.expectedSubject.length > 0
    && config.expectedSubject.length <= 256, 'owned_subject_required');
  check(marker(config.markers?.trigger) && marker(config.markers?.manual)
    && config.markers.trigger !== config.markers.manual, 'distinct_original_markers_required');
  for (const key of ['tokenFile', 'fixtureJournal', 'fixtureResult', 'outputDirectory']) {
    check(typeof config[key] === 'string' && isAbsolute(config[key]), 'absolute_private_paths_required');
  }
  check(config.browserExecutable === undefined || (typeof config.browserExecutable === 'string'
    && isAbsolute(config.browserExecutable)), 'absolute_browser_executable_required');
  check(Number.isInteger(config.limits?.turnSeconds) && config.limits.turnSeconds >= 30
    && config.limits.turnSeconds <= 1200 && Number.isInteger(config.limits.pollSeconds)
    && config.limits.pollSeconds >= 1 && config.limits.pollSeconds <= 30, 'bounded_polling_required');
  // Reuse the committed HTTPS, saved-token path and cluster-scope checks. Actual
  // Agent scope is established from the successful owned fixture before reads.
  validateConfig({ ...config, schema: 'cma-bridge-continuity-config-v1', agentIds: ['agt_owned_fixture_pending'] });
  return config;
}

export async function loadOwnedConfig(path) {
  check(typeof path === 'string' && path.length > 0, 'configuration_argument_required');
  const config = validateOwnedConfig(JSON.parse(await readFile(path, 'utf8')));
  await mkdir(config.outputDirectory, { mode: 0o700, recursive: true });
  const stat = await lstat(config.outputDirectory);
  check(stat.isDirectory() && stat.uid === process.getuid() && !(stat.mode & 0o077), 'private_output_directory_required');
  const output = await realpath(config.outputDirectory);
  const repository = await realpath(fileURLToPath(new URL('../../', import.meta.url)));
  const local = relative(repository, output);
  check(local.startsWith(`..${sep}`) || isAbsolute(local), 'evidence_must_remain_outside_repository');
  return config;
}

export function validateOwnedFixture(config, journal, result) {
  check(journal?.schema === 'cma-public-canary-journal-v1' && journal.run === config.run
    && journal.owner === config.expectedSubject && journal.origin === config.origin
    && journal.result?.status === 'passed' && result?.status === 'passed', 'successful_original_owned_canary_required');
  check(digest(result.resources) === digest(journal.resources)
    && journal.resources?.agentIds?.length === 2
    && journal.resources.agentIds.every((value) => identifier(value, 'agt'))
    && new Set(journal.resources.agentIds).size === 2
    && identifier(journal.resources.sandboxId, 'sbx') && identifier(journal.resources.workspaceId, 'wks'), 'replacement_resource_scope_changed');
  check(journal.commands && !journal.commands['retire-trigger'] && !journal.commands['pause-sandbox'], 'original_canary_cleanup_started');
  return journal;
}

export function retainedCommand(prior, path, body, key) {
  check(!prior || (prior.path === path && prior.body === body && prior.key === key), 'retained_command_changed');
  check(!prior || prior.state === 'accepted', 'unsettled_command_requires_explicit_recovery');
  return prior?.value;
}
