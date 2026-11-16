import {contextFor} from './context.mjs';
import {need, digest, canonical} from './provenance.mjs';

export function cookieDigest(cookies) {
  need(Array.isArray(cookies) && cookies.length > 0, 'browser_cookie_jar_missing');
  // Only the digest leaves this function. Values are never recorded.
  const selected = cookies.map(({name, value, domain, path, httpOnly, secure}) =>
    ({name, value, domain, path, httpOnly, secure}));
  need(selected.some(cookie => cookie.httpOnly && cookie.value), 'browser_session_cookie_missing');
  selected.sort((a, b) => canonical(a).localeCompare(canonical(b)));
  return digest(canonical(selected));
}

export async function verifyBrowserSession(ctx, session, identity) {
  const {binding, release} = contextFor(ctx);
  need(session?.origin === binding.web_origin && typeof session.page?.request?.get === 'function'
    && typeof session.page?.context === 'function', 'browser_session_transport_missing');
  const browser = ctx.fixture('browser'), evidence = await browser.evidence(session.page);
  need(evidence?.schema === 'multi-am-browser-evidence-v1' && evidence.real_ui === true
    && evidence.secrets_redacted === true && evidence.authentication === 'browser_session',
  'browser_cookie_authentication_required');
  need(evidence.compiled_source === release.web.source
    && evidence.compiled_manifest === release.web.manifest, 'browser_release_mismatch');
  // Playwright's context request shares the actual cookie jar. No bearer/header
  // override or browser route mock is used by this request.
  const response = await session.page.request.get(new URL('/auth/session', session.origin).href,
    {maxRedirects: 0, timeout: Math.max(1, Math.min(8000, ctx.remainingMs()))});
  need(response.status() === 200, 'browser_auth_session_status');
  const auth = await response.json();
  need(auth?.authenticated === true && typeof auth.subject === 'string'
    && digest(auth.subject) === identity.registration.subject_sha256, 'browser_subject_mismatch');
  const cookies = await session.page.context().cookies(session.origin);
  const cookie_jar_sha256 = cookieDigest(cookies);
  const session_binding_sha256 = digest(canonical({subject_sha256: digest(auth.subject),
    cookie_jar_sha256, origin: session.origin}));
  need(cookie_jar_sha256 === identity.session.cookie_jar_sha256
    && session_binding_sha256 === identity.session.binding_sha256, 'browser_registration_session_mismatch');
  const build = await session.page.request.get(new URL('/build-info.json', session.origin).href,
    {maxRedirects: 0, timeout: Math.max(1, Math.min(8000, ctx.remainingMs()))});
  need(build.status() === 200 && digest(await build.body()) === release.web.build_info_sha256,
    'browser_build_info_mismatch');
  return {...evidence, subject_sha256: digest(auth.subject), session_binding_sha256,
    registration_receipt_sha256: identity.receipt_sha256};
}

export async function observeBrowser(ctx, label, session, identity) {
  const evidence = await verifyBrowserSession(ctx, session, identity);
  await ctx.observe(label, async () => evidence);
  return evidence;
}
