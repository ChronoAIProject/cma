import { readFile } from 'node:fs/promises';
import { check, hash, isHash } from './checks.mjs';
import { catalogSha256 } from './catalog.mjs';

export function localUrl(value) {
  let url;
  try { url = new URL(value); } catch { check(false, 'invalid_local_url', true); }
  check(['http:', 'https:', 'ws:', 'wss:'].includes(url.protocol)
    && ['127.0.0.1', '[::1]'].includes(url.hostname)
    && !url.username && !url.password && !url.hash, 'nonlocal_target_refused', true);
  return url;
}

export function validateConfig(config) {
  check(config?.schema === 'cma-scale-v1' && config.environment === 'local', 'local_config_required', true);
  check(config.context === 'kind-cma-scale' && config.namespace === 'cma-scale', 'cluster_scope_refused', true);
  check(config.isolated === true, 'isolated_environment_required', true);
  check(config.catalogSha256 === catalogSha256, 'frozen_catalog_required', true);
  check(config.endpoints && Object.keys(config.endpoints).length >= 1, 'endpoints_required', true);
  for (const endpoint of Object.values(config.endpoints)) {
    const url = localUrl(endpoint);
    check(['http:', 'https:'].includes(url.protocol) && url.pathname === '/' && !url.search,
      'origin_required', true);
  }
  for (const key of ['hardwareHash', 'fixtureHash', 'workloadHash', 'sourceHash', 'imagesHash']) {
    check(isHash(config[key]), 'provenance_hash_required', true);
  }
  check(Number.isSafeInteger(config.recoveryMs) && config.recoveryMs > 0 && config.recoveryMs <= 30000,
    'invalid_recovery_bound', true);
  check(Number.isSafeInteger(config.scaleTimeoutMs) && config.scaleTimeoutMs >= 1000
    && config.scaleTimeoutMs <= 1800000, 'invalid_scale_timeout', true);
  return config;
}

export async function pinnedModule(path, expectedHash, importer = (url) => import(url)) {
  check(isHash(expectedHash), 'adapter_pin_required', true);
  check(hash(await readFile(path)) === expectedHash, 'adapter_pin_mismatch', true);
  return importer(path);
}

export function guardedFetch(fetchImpl = fetch) {
  return async (input, options = {}) => {
    localUrl(input);
    check(options.redirect === undefined || options.redirect === 'error', 'redirect_override_refused');
    return fetchImpl(input, { ...options, redirect: 'error' });
  };
}
