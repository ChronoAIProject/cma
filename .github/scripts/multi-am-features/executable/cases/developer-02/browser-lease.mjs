import {createBrowserAdapter} from '../../browser.mjs';
import {need} from './helpers.mjs';

// This adapter consumes a parent-owned browser; it never launches a browser,
// Web server, AM, provider or second topology. The parent's Web auth proxy must
// expose a real session for the selected actor at this origin.
export function browserFromLease(bindings) {
  need(bindings?.scope === 'LOCAL_DISPOSABLE_TWO_AM' && bindings.parentOwnsBrowser === true,
    'ui_parent_browser_lease_missing');
  need(bindings.browser?.newContext && bindings.actors?.user?.storageState
    && bindings.actors.user.sessionOrigin && bindings.actors.user.subject,
  'ui_auth_session_binding_missing');
  const adapter = createBrowserAdapter(bindings);
  return {...adapter, async open(options) {
    const identity = bindings.actors?.[options.actor];
    need(identity?.storageState && identity.sessionOrigin && identity.subject, 'ui_actor_session_missing');
    const session = await adapter.open(options);
    try {
      // Prove the relocated cookie session; a bearer on API requests alone is
      // insufficient. This is a real request through the selected AM proxy.
      const response = await session.page.request.get(session.origin + '/auth/session');
      const actual = response.status() === 200 ? await response.json() : null;
      if (actual?.authenticated !== true || actual.subject !== identity.subject)
        throw Error('ui_cookie_session_actor_mismatch');
      return session;
    } catch (error) {await session.close(); throw error;}
  }};
}
