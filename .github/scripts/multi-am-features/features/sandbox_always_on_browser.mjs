import {isDeepStrictEqual as equal} from 'node:util';
import {assertOwner, check, hash, id, other} from '../cma/core.mjs';

const onText = 'An administrator approved Always on, so automatic pause skips this Sandbox.';
const offText = 'Always on is off. Only an administrator can approve an exception.';
const buttonName = {name: 'Turn off Always on', exact: true};

function sandboxResponse(response, method, path) {
  return response.request().method() === method && new URL(response.url()).pathname === path;
}

async function requestDigest(response) {
  const requestId = await response.headerValue('x-request-id');
  check(typeof requestId === 'string' && requestId.trim().length > 0, 'always_on_browser_request_id_missing');
  return hash(requestId);
}

async function openSandbox({page, origin}, sandboxId, expectedRevision, enabled) {
  check(id(sandboxId) && Number.isSafeInteger(expectedRevision), 'always_on_browser_lineage_missing');
  const route = '/sandboxes/' + sandboxId;
  // Route changes can abort an in-flight read before its response body exists.
  const [request] = await Promise.all([
    page.waitForEvent('requestfinished', {predicate: request => request.method() === 'GET'
      && new URL(request.url()).pathname === '/api/v1' + route, timeout: 30000}),
    page.goto(route, {waitUntil: 'domcontentloaded', timeout: 20000}),
  ]);
  const response = await request.response();
  check(response?.status() === 200 && page.url() === origin + route, 'always_on_browser_read_failed');
  const body = await response.json();
  check(body.id === sandboxId && body.revision === expectedRevision && body.always_on === enabled,
    'always_on_browser_read_lineage_changed');
  await page.getByText(enabled ? onText : offText, {exact: true}).waitFor({state: 'visible', timeout: 15000});
  return requestDigest(response);
}

export async function relinquishAlwaysOnInBrowser(browser, ingress, sandboxId, revision) {
  return browser.forIngress(ingress, async tab => {
    const read = await openSandbox(tab, sandboxId, revision, true);
    const button = tab.page.getByRole('button', buttonName);
    const [response] = await Promise.all([
      tab.page.waitForResponse(value => sandboxResponse(value, 'POST', `/api/v1/sandboxes/${sandboxId}/always-on`),
        {timeout: 30000}),
      button.click({timeout: 15000}),
    ]);
    check(response.status() === 200, 'always_on_browser_relinquishment_failed');
    const request = response.request();
    const body = request.postDataJSON();
    const key = await request.headerValue('idempotency-key');
    check(body?.always_on === false && body.expected_revision === revision && typeof key === 'string' && key.length > 0,
      'always_on_browser_intent_unbound');
    await tab.page.getByText(offText, {exact: true}).waitFor({state: 'visible', timeout: 15000});
    await button.waitFor({state: 'hidden', timeout: 15000});
    const mutation = await requestDigest(response);
    check(mutation !== read, 'always_on_browser_request_id_reused');
    return {intent: {body, key}, receipt: await response.json(), read, mutation};
  });
}

export function observeAlwaysOnOffInBrowser(browser, ingress, sandboxId, revision) {
  return browser.forIngress(ingress, async tab => {
    const read = await openSandbox(tab, sandboxId, revision, false);
    await tab.page.getByRole('button', buttonName).waitFor({state: 'hidden', timeout: 15000});
    return read;
  });
}

export async function relinquishAndReplayAlwaysOn({browser, user, adapter, agentId, sandboxId, revision}) {
  const owner = assertOwner(await adapter.ownerOf({agentId}), agentId);
  const ingress = other(owner.am);
  const {intent, receipt, read, mutation} = await relinquishAlwaysOnInBrowser(browser, ingress, sandboxId, revision);
  check(receipt?.id === sandboxId && receipt.always_on === false && receipt.revision === revision + 1,
    'owner_relinquishment_unconfirmed');
  const {body: replayed, timing} = await user.request(owner.am, 'POST', `/api/v1/sandboxes/${sandboxId}/always-on`, intent);
  check(equal(replayed, receipt), 'owner_relinquishment_replay_changed');
  check(typeof timing?.request_id === 'string' && timing.request_id.trim().length > 0,
    'owner_relinquishment_replay_unbound');
  const peerRead = await observeAlwaysOnOffInBrowser(browser, owner.am, sandboxId, receipt.revision);
  const requests = {approved_read: read, relinquishment: mutation, replay: hash(timing.request_id), peer_read: peerRead};
  check(new Set(Object.values(requests)).size === 4, 'owner_relinquishment_request_id_reused');
  const after = assertOwner(await adapter.ownerOf({agentId}), agentId);
  check(after.am === owner.am && after.epoch === owner.epoch && after.pod_uid === owner.pod_uid,
    'owner_changed_during_browser_relinquishment');
  return {ingress, owner: owner.am, epoch: owner.epoch, request_sha256: requests,
    receipt_sha256: hash(JSON.stringify(receipt)), revisions_advanced: receipt.revision - revision};
}
