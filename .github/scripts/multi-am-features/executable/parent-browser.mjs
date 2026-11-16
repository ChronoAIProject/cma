// Invoked only inside the sole admitted parent. Its caller owns finalization.
import {readFile, realpath} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import http from 'node:http';
import path from 'node:path';

const check = (condition, code) => {if (!condition) throw Error(code);};
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const types = {'.html':'text/html; charset=utf-8','.js':'text/javascript; charset=utf-8',
  '.css':'text/css; charset=utf-8','.json':'application/json','.svg':'image/svg+xml',
  '.png':'image/png','.woff2':'font/woff2','.ico':'image/x-icon'};

export async function createParentBrowserLease({config, launchBrowser, actors, adapter, resolveAgent}) {
  check(config.scope === 'LOCAL_DISPOSABLE_TWO_AM' && config.database_disposable === true
    && config.single_matrix_writer === true, 'browser_parent_custody_required');
  check(typeof launchBrowser === 'function' && typeof adapter?.topology === 'function'
    && typeof resolveAgent === 'function', 'browser_parent_hooks_required');
  const compiled = config.compiled_web;
  check(/^[a-f0-9]{40}$/.test(compiled?.source ?? '')
    && /^sha256:[a-f0-9]{64}$/.test(compiled?.image ?? '')
    && Array.isArray(compiled.files) && compiled.files.length > 0, 'browser_exact_assets_required');
  check(actors?.user?.storageState && actors.user.sessionOrigin && actors.user.subject,
    'browser_real_cookie_session_required');
  for (const actor of Object.values(actors)) {
    check(!actor.bearer && actor.storageState?.cookies?.some(c => c.name === 'cma_session' && c.value)
      && actor.subject && actor.sessionOrigin === 'https://api.localhost.direct',
    'browser_exact_cookie_actor_required');
  }
  const root = await realpath(compiled.asset_root), files = new Map();
  for (const entry of compiled.files) {
    const resolved = await realpath(path.resolve(root, entry.path));
    check(resolved.startsWith(root + path.sep), 'browser_asset_escape');
    const bytes = await readFile(resolved);
    check(hash(bytes) === entry.sha256 && !files.has('/' + entry.path), 'browser_asset_hash_mismatch');
    files.set('/' + entry.path, bytes);
  }
  check(files.has('/index.html') && files.has('/build-info.json'), 'browser_asset_entrypoints_missing');
  const build = JSON.parse(files.get('/build-info.json'));
  check(build.revision === compiled.source && build.version === compiled.build_version
    && build.local === false, 'browser_compiled_build_identity');
  const topology = await adapter.topology();
  const ams = {};
  for (const am of ['a','b']) {
    const member = topology.ams?.[am];
    check(member?.ready === true && member.image === config.expected_api_image
      && member.pod_uid && member.instance_id && member.container_id, 'browser_ready_exact_am_required');
    const url = new URL(member.origin);
    check(url.protocol === 'http:' && ['127.0.0.1','localhost'].includes(url.hostname)
      && !url.username && !url.password && url.pathname === '/' && !url.search && !url.hash,
    'browser_local_am_required');
    ams[am] = url.origin;
  }
  check(ams.a !== ams.b && topology.ams.a.pod_uid !== topology.ams.b.pod_uid
    && topology.ams.a.instance_id !== topology.ams.b.instance_id, 'browser_two_distinct_ams_required');
  let browser, closing;
  const sockets = new Set();
  const server = http.createServer((request, response) => {
    if (!['GET','HEAD'].includes(request.method)) {response.writeHead(405).end();return;}
    let name;
    try {name = decodeURIComponent(new URL(request.url, 'http://assets.invalid').pathname);}
    catch {response.writeHead(400).end();return;}
    if (/^\/(api|auth|\.well-known)(\/|$)/.test(name)) {response.writeHead(404).end();return;}
    if (!files.has(name)) {
      if (path.extname(name)) {response.writeHead(404).end();return;}
      name = '/index.html';
    }
    const bytes = files.get(name);
    response.writeHead(200, {'Content-Type':types[path.extname(name)] ?? 'application/octet-stream',
      'Content-Length':bytes.length, 'Cache-Control':'no-store'});
    response.end(request.method === 'HEAD' ? undefined : bytes);
  });
  server.on('connection', socket => {sockets.add(socket);socket.once('close', () => sockets.delete(socket));});
  const close = () => closing ??= (async () => {
    for (const socket of sockets) socket.destroy();
    const results = await Promise.allSettled([
      Promise.resolve().then(() => browser?.close()), new Promise((resolve, reject) => server.close(error => {
        if (error && error.code !== 'ERR_SERVER_NOT_RUNNING') reject(error);else resolve();
      })),
    ]);
    const errors = results.filter(r => r.status === 'rejected').map(r => r.reason);
    if (errors.length) throw new AggregateError(errors, 'browser_parent_release_failed');
  })();
  try {
    await new Promise((resolve, reject) => {server.once('error', reject);server.listen(0,'127.0.0.1',resolve);});
    browser = await launchBrowser();
    check(typeof browser?.newContext === 'function' && typeof browser?.close === 'function',
      'browser_parent_process_required');
    const webOrigin = 'http://127.0.0.1:' + server.address().port;
    return {lease:{scope:config.scope,parentOwnsBrowser:true,browser,webOrigin,ams,actors,adapter,
      resolveAgent,compiledSource:compiled.source,compiledManifest:compiled.image,
      host:'api.localhost.direct',publicOrigin:'https://api.localhost.direct'},close};
  } catch (error) {
    try {await close();} catch (cleanup) {error.cleanup_error = cleanup;}
    throw error;
  }
}

// Called once by the admitted parent before runPhase. Nothing is launched on
// import, and the parent supplies the only browser-launch hook.
export async function installParentBrowserLease({services, ...options}) {
  check(services && !services.executable?.browserLease, 'browser_parent_already_installed');
  const owned = await createParentBrowserLease(options), previous = services.executable;
  const compiled = options.config.compiled_web;
  const installed = {...previous, browserLease: owned.lease,
    primitives: {...previous?.primitives,
      compiledWeb: {source: compiled.source, manifestSha256: compiled.image,
        buildInfoSha256: compiled.files.find(file => file.path === 'build-info.json').sha256},
      identities: Object.fromEntries(Object.entries(options.actors).map(([actor, value]) =>
        [actor, {actor, subject: value.subject}])),
    }};
  services.executable = installed;
  return {async close() {
    try {await owned.close();}
    finally {if (services.executable === installed) services.executable = previous;}
  }};
}
