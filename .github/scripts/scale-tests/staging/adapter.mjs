import { createRequire } from 'node:module';
import { readFile, realpath } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';
import { check, hash, safeFailure } from '../lib/checks.mjs';
import { privateBytes, stagingEnvironment, validateConfig, validateAuthority } from './authority.mjs';
import { sourceHash } from './prepare.mjs';
import { BrowserTransport } from './browser.mjs';
import { Evidence } from './evidence.mjs';
import { StagingRun } from './run.mjs';

const requireTool = createRequire(import.meta.url);
// Adapter only: scheduling and the independently monitored execution window belong to the caller.
export async function withStagingDriver(authorization, directory, use) {
  const evidence = await Evidence.open(directory);
  let browser; let run; let timer; let signal;
  try {
    check(typeof authorization?.sha256 === 'string', 'approved_config_hash_required', true);
    const configBytes = await privateBytes(authorization.path, authorization.sha256);
    const config = validateConfig(JSON.parse(configBytes));
    config.configHash = hash(configBytes);
    check(config.driverHash === await sourceHash(), 'reviewed_driver_changed', true);
    await privateBytes(config.wire.path, config.wire.sha256);
    check(config.browser.path === await realpath(config.browser.path)
      && hash(await readFile(config.browser.path)) === config.browser.sha256, 'browser_binary_changed', true);
    const states = new Map();
    for (const f of config.fixtures) states.set(f.subject,
      JSON.parse(await privateBytes(f.storageStatePath, f.storageStateHash)));
    const readAuthority = async () => JSON.parse(await privateBytes(config.authorityPath));
    validateAuthority(config, await readAuthority());
    const wire = await import(pathToFileURL(config.wire.path).href);
    check(typeof wire.agentFrames === 'function' && typeof wire.v2AgentEventHead === 'function', 'parser_exports_missing');
    const { chromium } = requireTool('@playwright/test');
    browser = await chromium.launch({ executablePath: config.browser.path, headless: true,
      env: stagingEnvironment(process.env) });
    run = new StagingRun(config, { wire, readAuthority, evidence, connect: async (spec) => {
      const context = await browser.newContext({ storageState: states.get(spec.subject), serviceWorkers: 'block' });
      const http = new BrowserTransport(context, config.origin, spec.subject, (code) => run.stop(code),
        () => run.quota(spec, 'read', run.time.now() + 30000));
      try { await http.initialize(); return http; }
      catch (error) { await context.close(); throw error; }
    } });
    signal = () => run.stop('operator_signal');
    process.on('SIGINT', signal); process.on('SIGTERM', signal);
    timer = setInterval(() => { void run.authority().catch(() => run.stop('authority_unavailable')); }, 1000);
    const result = await run.scope(use);
    return await evidence.finish(result);
  } catch (error) {
    evidence.record({ scenario: 'preflight_or_finalization', ...safeFailure(error) });
    return await evidence.finish({ cleanup: false });
  } finally {
    clearInterval(timer);
    if (signal) { process.off('SIGINT', signal); process.off('SIGTERM', signal); }
    await browser?.close();
  }
}
