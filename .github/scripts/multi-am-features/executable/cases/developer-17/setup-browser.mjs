import { Blocked } from '../../runner.mjs';
import { digest } from './helpers.mjs';

export function identity(primitives, actor) {
  const value = primitives.identities?.[actor];
  if (value?.actor !== actor || typeof value.subject !== 'string' || !value.subject.trim())
    throw new Blocked('user_setup_identity_unavailable');
  return value;
}

// These are independent parent artifact identities, never values read back from
// the UI under test. Only the served build-info bytes determine the display data.
export function requireBrowserInputs(primitives, actors = ['user']) {
  const build = primitives.compiledWeb;
  if (
    !/^[a-f0-9]{40}$/.test(build?.source ?? '') ||
    !/^sha256:[a-f0-9]{64}$/.test(build?.manifestSha256 ?? '') ||
    !/^[a-f0-9]{64}$/.test(build?.buildInfoSha256 ?? '')
  )
    throw new Blocked('compiled_web_artifact_identity_required');
  const identities = actors.map((actor) => identity(primitives, actor));
  if (new Set(identities.map((value) => value.subject)).size !== identities.length)
    throw new Blocked('user_setup_distinct_subjects_required');
}

export async function prepareBrowser(
  ctx,
  primitives,
  { actor = 'user', agentId, clipboard = false } = {},
) {
  requireBrowserInputs(primitives, [actor]);
  const browser = ctx.fixture('browser');
  const session = await browser.open({ actor, am: ctx.am, agentId });
  let closed;
  const cleanup = () => (closed ??= session.close());
  ctx.defer('close_setup_browser', cleanup);
  if (
    typeof session.close !== 'function' ||
    typeof session.switchAM !== 'function' ||
    !session.page ||
    !session.origin
  )
    throw new Blocked('real_browser_session_required');
  const evidence = await browser.evidence(session.page);
  ctx.assert(
    evidence.schema === 'multi-am-browser-evidence-v1' &&
      evidence.real_ui === true &&
      evidence.secrets_redacted === true &&
      evidence.compiled_source === primitives.compiledWeb.source &&
      evidence.compiled_manifest === primitives.compiledWeb.manifestSha256,
    'setup_compiled_web_binding',
  );
  if (evidence.authentication !== 'browser_session')
    throw new Blocked('browser_session_authentication_required');
  const authResponse = await session.page.request.get(
    new URL('/auth/session', session.origin).href,
  );
  ctx.assert(authResponse.status() === 200, 'setup_session_status');
  const auth = await authResponse.json();
  ctx.assert(
    auth.authenticated === true &&
      auth.subject === identity(primitives, actor).subject &&
      Array.isArray(auth.capabilities) &&
      auth.capabilities.every((value) => typeof value === 'string'),
    'setup_session_identity',
  );
  const response = await session.page.request.get(new URL('/build-info.json', session.origin).href);
  ctx.assert(response.status() === 200, 'setup_build_info_status');
  const bytes = await response.body();
  ctx.assert(digest(bytes) === primitives.compiledWeb.buildInfoSha256, 'setup_build_info_digest');
  const build = JSON.parse(bytes.toString('utf8'));
  ctx.assert(
    typeof build.version === 'string' &&
      build.version.trim().length > 0 &&
      (build.revision === primitives.compiledWeb.source ||
        (build.local === true && build.revision === '')) &&
      typeof build.local === 'boolean',
    'setup_build_identity',
  );
  // The initial Playwright page is about:blank, so establish a real secure origin.
  await session.page.goto(new URL('/agent-manager', session.origin).href);
  if (clipboard) {
    const granted = await session.page.evaluate(async () =>
      Promise.all(
        ['clipboard-read', 'clipboard-write'].map(
          async (name) => (await navigator.permissions.query({ name })).state,
        ),
      ),
    );
    if (granted.length !== 2 || granted.some((value) => value !== 'granted'))
      throw new Blocked('browser_clipboard_permissions_required');
  }
  await ctx.observe('setup_browser_identity', async () => ({
    actor,
    subject_sha256: digest(auth.subject),
    compiled_source: evidence.compiled_source,
    compiled_manifest: evidence.compiled_manifest,
    build_info_sha256: digest(bytes),
    authentication: evidence.authentication,
    clipboard_checked: clipboard,
  }));
  return {
    session: { ...session, close: cleanup },
    cleanup,
    actor,
    capabilities: [...auth.capabilities],
    webBuild: {
      provenance: 'compiled-web-manifest',
      label: build.local ? `${build.version} (local)` : build.version,
      copyText: `CMA ${build.version} (${build.revision || 'local'})${build.local && build.revision ? ' (local)' : ''}`,
    },
  };
}
