import {readFileSync} from 'node:fs';
import {realpath} from 'node:fs/promises';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {createHash} from 'node:crypto';
import {fileURLToPath} from 'node:url';
import {isAbsolute} from 'node:path';
import {Blocked} from '../../runner.mjs';
import {check, equal} from './common.mjs';
import {writableSettings, publicSetups} from './public-setups.mjs';
import {publishedProfile} from './settings-profile.mjs';

const run = promisify(execFile);
const root = fileURLToPath(new URL('../../../../../../', import.meta.url));
const contract = JSON.parse(readFileSync(new URL('./reset-contract.json', import.meta.url)));
const verified = new Set();

// Reset expectations come from the reviewed Rust defaults AND the public DTO,
// not from the reset response that the case is meant to verify. Other images
// must carry byte-identical contracts or receive an explicit fixture update.
export async function resetDefaults(source, repository = root) {
  if (!/^[a-f0-9]{40}$/.test(source ?? '')) throw new Blocked('admin_reset_source_unbound');
  if (typeof repository !== 'string' || !isAbsolute(repository)) {
    throw new Blocked('admin_reset_repository_unbound');
  }
  let directory;
  try {directory = await realpath(repository);} catch {throw new Blocked('admin_reset_source_unavailable');}
  const key = directory + '\0' + source;
  if (!verified.has(key)) {
    for (const [path, expected] of Object.entries(contract.files)) {
      let stdout;
      try {({stdout} = await run('git', ['show', source + ':' + path], {
        cwd: directory, timeout: 5000, maxBuffer: 1024 * 1024, encoding: 'buffer',
      }));} catch {throw new Blocked('admin_reset_source_unavailable');}
      if (createHash('sha256').update(stdout).digest('hex') !== expected) {
        throw new Blocked('admin_reset_source_contract_changed');
      }
    }
    verified.add(key);
  }
  return structuredClone(contract.defaults);
}

export async function settingsPatchSetup(ctx, {config}) {
  // Frozen coordinators have no Git metadata. The parent may bind a read-only
  // source checkout; Git still reads the exact image commit, never its worktree.
  const defaults = await resetDefaults(config.expected_api_source, config.admin_source_repository);
  await ctx.observe('admin_reset_source_contract', async () => ({
    source: config.expected_api_source, files: {...contract.files},
  }));
  const current = await writableSettings(ctx, true), s = current.settings;
  // The global parent lease does not stop background scanners or existing model
  // work. This fixture only runs with both autonomous policy systems disabled.
  if (s.inactivity.auto_pause_enabled || s.triggers.enabled || s.model_budget?.enabled) {
    throw new Blocked('admin_patch_requires_disabled_background_policies');
  }
  if (s.model_budget?.platform?.cloud && current.platform_spend_writes_available !== true) {
    throw new Blocked('admin_patch_cloud_writer_unavailable');
  }
  const {period} = await publicSetups.spend_period(ctx);
  const profile = await publishedProfile(ctx);
  const different = (value, a, b) => value === a ? b : a;
  const patchSections = [
    {section: 'model_budget', value: {...s.model_budget, platform: period}},
    {section: 'usage_reporting', value: {measured_visible: !s.usage_reporting.measured_visible}},
    {section: 'defaults', value: {...s.defaults, agent_profile: profile.reference}},
    {section: 'inactivity', value: {...s.inactivity,
      pause_after_seconds: different(s.inactivity.pause_after_seconds, 900, 901)}},
    {section: 'retention', value: {agent_log_days: different(s.retention.agent_log_days, 30, 31)}},
    {section: 'limits', value: {...s.limits,
      max_sandboxes_per_user: different(s.limits.max_sandboxes_per_user, 10, 11)}},
    {section: 'triggers', value: {...s.triggers, orchestration_paused: !s.triggers.orchestration_paused}},
    {section: 'marketplace', value: {enabled: !s.marketplace.enabled}},
  ];
  check(patchSections.every(({section, value}) => !equal(s[section], value)), 'admin_patch_setup_unchanged');
  // Every settings read is itself audited; immutable local rows are removed by
  // the parent's final database disposal, not by manual database edits here.
  await ctx.observe('admin_patch_database_finality', async () => ({
    final_database_cleanup: 'PARENT_MUST_CONFIRM_BEFORE_CAMPAIGN_PASS',
  }));
  return {resetDefaults: defaults, patchSections, cleanup: profile.cleanup};
}
