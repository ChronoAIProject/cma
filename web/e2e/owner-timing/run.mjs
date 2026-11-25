import { lstat, mkdir, readFile, realpath, rm } from 'node:fs/promises';
import { resolve, dirname, basename, relative, isAbsolute } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { randomUUID } from 'node:crypto';
import process from 'node:process';
import { chromium } from 'playwright';
import {
  cases,
  hash,
  publicRead,
  requireCheck,
  Results,
  safeFailure,
} from '../../../.github/scripts/scale-tests/timing/lib/core.mjs';
import {
  guardPage,
  openAgent,
  OwnedResources,
  sendMessage,
  session,
  stopMessage,
} from '../../../.github/scripts/scale-tests/timing/lib/flows.mjs';
import { measureAdminReport } from './admin.mjs';

const shaPattern = /^[a-f0-9]{40}$/;
const repository = fileURLToPath(new URL('../../../', import.meta.url));

export function parseConfig(input) {
  const target = new URL(input.origin);
  const loopback = ['127.0.0.1', '[::1]'].includes(target.hostname);
  requireCheck(
    !target.username &&
      !target.password &&
      target.href === target.origin + '/' &&
      (target.protocol === 'https:' || (target.protocol === 'http:' && loopback)),
    'origin_invalid',
  );
  const environment = input.environment;
  requireCheck(
    ['fixture', 'staging', 'production'].includes(environment) &&
      (environment === 'fixture' ? loopback : !loopback),
    'environment_invalid',
  );
  requireCheck(['probe', 'measure'].includes(input.mode), 'mode_invalid');
  const attempts = input.attempts ?? 10;
  requireCheck(Number.isInteger(attempts) && attempts >= 10, 'attempts_invalid');
  requireCheck(typeof input.output === 'string' && isAbsolute(input.output), 'output_path_invalid');
  if (input.mode === 'measure') {
    requireCheck(
      input.approve_create_cleanup === true &&
        input.approve_admin_report === true &&
        /^[a-f0-9]{64}$/.test(input.subject_sha256 ?? '') &&
        isAbsolute(input.storage_state ?? '') &&
        shaPattern.test(input.web_sha ?? '') &&
        shaPattern.test(input.am_sha ?? ''),
      'measurement_authority_incomplete',
    );
  }
  return { ...input, origin: target.origin, attempts };
}

async function privateStorage(path) {
  const file = await lstat(path);
  requireCheck(
    file.isFile() &&
      (file.mode & 0o077) === 0 &&
      file.uid === process.getuid() &&
      (await realpath(path)) === path,
    'storage_state_not_private',
  );
  return JSON.parse(await readFile(path, 'utf8'));
}

async function sameBuild(config) {
  const web = await publicRead(config.origin, '/build-info.json');
  const am = await publicRead(config.origin, '/api/v1/build-info');
  requireCheck(
    web.status === 200 &&
      am.status === 200 &&
      JSON.parse(web.body).revision === config.web_sha &&
      JSON.parse(am.body).source_sha === config.am_sha,
    'serving_revision_mismatch',
  );
}

async function measureProbes(config, results) {
  for (let attempt = 1; attempt <= config.attempts; attempt += 1) {
    await results.measure('public_build_info', attempt, async () => {
      const response = await publicRead(config.origin, '/build-info.json');
      requireCheck(response.status === 200, 'build_info_http_failed', response.status);
      const revision = JSON.parse(response.body).revision;
      requireCheck(shaPattern.test(revision ?? ''), 'build_info_invalid');
      return { http_status: 200, web_sha: revision };
    });
    await results.measure('signed_out_session', attempt, async () => {
      const response = await publicRead(config.origin, '/auth/session');
      const signedOut =
        response.status === 401 ||
        (response.status === 200 && JSON.parse(response.body).authenticated === false);
      requireCheck(signedOut, 'signed_out_boundary_unverified', response.status);
      return { http_status: response.status };
    });
  }
}

async function measureAgent(page, config, results, attempt, agentId) {
  await guardPage(page, config.origin, agentId, { messages: true });
  const opened = await results.measure('agent_open', attempt, () =>
    openAgent(page, config.origin, agentId),
  );
  if (!opened) {
    results.block('message_send', 'interactive_agent_unavailable', attempt);
    results.block('message_stop', 'interactive_agent_unavailable', attempt);
    return;
  }

  let receipt;
  const sent = await results.measure('message_send', attempt, async () => {
    const marker = randomUUID();
    const prompt = 'For this timing check, count from 1 through 2000 and end with ' + marker + '.';
    receipt = await sendMessage(page, config.origin, agentId, prompt);
    return receipt.evidence;
  });
  if (sent) {
    await results.measure('message_stop', attempt, () =>
      stopMessage(page, config.origin, agentId, receipt.responseId),
    );
  } else results.block('message_stop', 'matching_send_receipt_unavailable', attempt);
}

async function measureCreate(context, config, results, attempt, desktop) {
  const variant = desktop ? 'create_desktop' : 'create_plain';
  const page = await context.newPage();
  const journal = resolve(config.output, 'cleanup-' + attempt + '-' + variant + '.json');
  const owned = new OwnedResources(context, config.origin, journal);
  try {
    const created = await results.measure(variant, attempt, () => owned.create(desktop));
    if (!desktop && created)
      await measureAgent(page, config, results, attempt, owned.record.agentId);
    if (!desktop && !created) {
      for (const name of ['agent_open', 'message_send', 'message_stop'])
        results.block(name, 'ready_agent_unavailable', attempt);
    }
  } finally {
    try {
      await page.close();
    } finally {
      await measureIdentity(context, config);
      await owned.cleanup();
      await rm(journal, { force: true });
    }
  }
}

async function measureIdentity(context, config) {
  const identity = await session(context, config.origin);
  requireCheck(hash(identity.subject) === config.subject_sha256, 'approved_subject_mismatch');
  return identity;
}

async function measureOwner(config, results) {
  const storageState = await privateStorage(config.storage_state);
  const browser = await chromium.launch(
    config.browser_executable ? { executablePath: config.browser_executable } : {},
  );
  try {
    const context = await browser.newContext({ storageState, serviceWorkers: 'block' });
    for (let attempt = 1; attempt <= config.attempts; attempt += 1) {
      await sameBuild(config);
      const identity = await measureIdentity(context, config);
      requireCheck(
        identity.capabilities?.includes('read_all_sandboxes'),
        'admin_report_capability_missing',
      );
      await measureCreate(context, config, results, attempt, false);
      await measureCreate(context, config, results, attempt, true);
      const page = await context.newPage();
      try {
        await results.measure('admin_report', attempt, () =>
          measureAdminReport(page, config.origin),
        );
      } finally {
        await page.close();
      }
    }
    await sameBuild(config);
  } finally {
    await browser.close();
  }
  results.meta.revision_confirmed = true;
}

export async function run(input) {
  const config = parseConfig(input);
  config.output = resolve(await realpath(dirname(config.output)), basename(config.output));
  const distance = relative(await realpath(repository), config.output);
  requireCheck(
    distance.startsWith('../') || isAbsolute(distance),
    'output_must_be_outside_repository',
  );
  await mkdir(config.output, { mode: 0o700 });
  const results = new Results({
    environment: config.environment,
    mode: config.mode,
    attempts_per_case: config.attempts,
    started_at: new Date().toISOString(),
    origin: config.origin,
    ...(config.mode === 'measure' ? { web_sha: config.web_sha, am_sha: config.am_sha } : {}),
    revision_confirmed: false,
  });
  try {
    if (config.mode === 'probe') await measureProbes(config, results);
    else await measureOwner(config, results);
  } catch (error) {
    results.meta.run_failure = safeFailure(error);
  }
  for (const name of Object.keys(cases)) {
    if (config.mode === 'probe') {
      results.block(name, 'read_only_probe');
      continue;
    }
    const observed = new Set(
      results.samples.filter((sample) => sample.case === name).map((sample) => sample.run),
    );
    for (let attempt = 1; attempt <= config.attempts; attempt += 1) {
      if (!observed.has(attempt)) results.block(name, 'run_stopped_before_attempt', attempt);
    }
  }
  return results.save(config.output);
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  try {
    requireCheck(process.argv.length === 3, 'configuration_file_required');
    const report = await run(JSON.parse(await readFile(process.argv[2], 'utf8')));
    const { metadata, samples, summary } = report;
    if (
      metadata.run_failure ||
      samples.some((sample) => sample.outcome === 'failed') ||
      (metadata.mode === 'measure' &&
        Object.keys(cases).some(
          (name) =>
            summary[name].completed !== metadata.attempts_per_case || summary[name].breaches > 0,
        ))
    ) {
      process.exitCode = 1;
    }
  } catch (error) {
    console.error(safeFailure(error).code);
    process.exitCode = 1;
  }
}
