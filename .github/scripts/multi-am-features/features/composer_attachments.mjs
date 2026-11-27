import {createHash, randomUUID} from 'node:crypto';
import {isDeepStrictEqual as equal} from 'node:util';
import {assertOracle, check, id, other, poll} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';
import {isPreparedSend} from '../prepared_browser.mjs';

const timeout = 20000;
const markerPrefix =
  '\n\nAttached files are available in the sandbox. Read the relevant files at these paths; images can be opened with your image tool:\n';
const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+X3S8AAAAASUVORK5CYII=', 'base64');

export async function composerAttachments({page, api, adapter, ingress, agentId, recordFailure}) {
  check(id(agentId), 'browser_attachments_agent_required');
  const agent = '/agents/' + agentId;
  await page.goto(agent, {waitUntil: 'domcontentloaded', timeout});
  const origin = new URL(page.url()).origin;
  const path = '/api/v1' + agent + '/attachments';
  const files = '/api/v1' + agent + '/files';
  const composer = page.getByRole('form', {name: 'Message Agent', exact: true});
  const draft = composer.getByRole('textbox', {name: 'Message', exact: true});
  const send = composer.getByRole('button', {name: 'Send message', exact: true});
  const list = composer.getByRole('list', {name: 'Attachments', exact: true});
  const row = name => list.getByRole('listitem').filter({has: page.getByText(name, {exact: true})});
  const chooser = {name: 'chosen.txt', media: 'text/plain', bytes: Buffer.from('Keep the selected attachment.\n')};
  const pasted = {name: 'pasted.png', media: 'image/png', bytes: png};
  const dropped = {name: 'dropped.txt', media: 'text/plain', bytes: Buffer.from('Remove this draft attachment.\n')};
  const seen = [];
  const receipts = new Map();
  const matches = (request, name) => {
    const url = new URL(request.url());
    return request.method() === 'POST' && url.origin === origin && url.pathname === path
      && (name === undefined || url.searchParams.get('name') === name);
  };
  const observe = request => { if (matches(request)) seen.push(request); };
  const receipt = (response, body, file) => {
    check(response.status() === 201 && body?.agent_id === agentId && /^att_[a-f0-9]{64}$/.test(body.id)
      && body.name === file.name && body.path === `/home/attachments/${agentId}/${body.id}/${file.name}`
      && body.sha256 === createHash('sha256').update(file.bytes).digest('hex')
      && body.size_bytes === file.bytes.length && body.media_type === file.media,
    'browser_attachments_receipt_changed');
    receipts.set(body.path, body);
    return body;
  };
  const transfer = (event, {name, media, bytes}, size) => draft.evaluate((element, input) => {
    const data = new DataTransfer();
    data.items.add(new File([input.size === undefined ? new Uint8Array(input.bytes) : new Uint8Array(input.size)],
      input.name, {type: input.media}));
    const options = {bubbles: true, cancelable: true};
    element.dispatchEvent(input.event === 'paste'
      ? new ClipboardEvent('paste', {...options, clipboardData: data})
      : new DragEvent('drop', {...options, dataTransfer: data}));
  }, {event, name, media, bytes: Array.from(bytes), size});
  const awaitUpload = async (file, action) => {
    const [response] = await Promise.all([
      page.waitForResponse(reply => matches(reply.request(), file.name), {timeout}), action(),
    ]);
    const request = response.request();
    check(request.postDataBuffer()?.equals(file.bytes) === true && id(await request.headerValue('idempotency-key')),
      'browser_attachments_upload_intent_changed');
    const value = receipt(response, await response.json(), file);
    await row(file.name).getByRole('status').getByText('Ready', {exact: true}).waitFor({timeout});
    return value;
  };

  const caseId = 'browser-attachments-' + randomUUID();
  const marker = 'WS4_' + randomUUID().replaceAll('-', '');
  const prompt = 'Return the exact marker ' + marker + ' without tools or file changes.';
  let releaseUpload, firstReceipt, firstFailure, interruptedUpload;
  const waitForProgress = new Promise(resolve => { releaseUpload = resolve; });
  const chooserRoute = url => url.origin === origin && url.pathname === path
    && url.searchParams.get('name') === chooser.name;
  const loseReply = route => {
    interruptedUpload = (async () => {
      try {
        const response = await route.fetch({maxRedirects: 0, maxRetries: 0, timeout});
        firstReceipt = receipt(response, await response.json(), chooser);
        await waitForProgress;
      } catch (error) {
        firstFailure = error;
      } finally {
        await route.abort('failed');
      }
    })();
    return interruptedUpload;
  };
  const read = (am, target, allowed) => api.rawRequest(am, 'GET',
    files + '/content?path=' + encodeURIComponent(target.slice('/home/'.length)), {allowed});

  page.on('request', observe);
  await withRestoration(async () => {
    await draft.fill(prompt, {timeout});
    await page.route(chooserRoute, loseReply, {times: 1});
    const [filechooser] = await Promise.all([
      page.waitForEvent('filechooser', {timeout}),
      composer.getByRole('button', {name: 'Attach files', exact: true}).click({timeout}),
    ]);
    await filechooser.setFiles({name: chooser.name, mimeType: chooser.media, buffer: chooser.bytes}, {timeout});
    await row(chooser.name).getByRole('status').getByText('Uploading…', {exact: true}).waitFor({timeout});
    check(await send.isDisabled(), 'browser_attachments_pending_send_enabled');
    releaseUpload();
    const retry = row(chooser.name).getByRole('button', {name: 'Retry', exact: true});
    await retry.waitFor({timeout});
    if (firstFailure) throw firstFailure;
    const chosen = await awaitUpload(chooser, () => retry.click({timeout}));
    check(equal(firstReceipt, chosen) && seen.length === 2
      && await seen[0].headerValue('idempotency-key') === await seen[1].headerValue('idempotency-key'),
    'browser_attachments_retry_not_idempotent');
    const pastedReceipt = await awaitUpload(pasted, () => transfer('paste', pasted));
    await awaitUpload(dropped, () => transfer('drop', dropped));
    await row(dropped.name).getByRole('button', {name: 'Remove ' + dropped.name, exact: true}).click({timeout});
    await row(dropped.name).waitFor({state: 'hidden', timeout});

    const invalid = {...dropped, name: 'invalid\\name.txt'};
    await transfer('drop', invalid);
    await row(invalid.name).getByRole('status').getByText(
      'attachment name must be a file name without path separators or controls', {exact: true}).waitFor({timeout});
    await row(invalid.name).getByRole('button', {name: 'Remove ' + invalid.name, exact: true}).click({timeout});
    await transfer('drop', {...dropped, name: 'oversize.txt'}, 20 * 1024 * 1024 + 1);
    await composer.getByRole('alert').getByText('Each attachment must be 20 MiB or smaller.', {exact: true}).waitFor({timeout});
    check(seen.length === 4, 'browser_attachments_invalid_input_dispatched');

    const kept = [chosen, pastedReceipt];
    for (const [item, file] of [[chosen, chooser], [pastedReceipt, pasted]]) {
      for (const am of [ingress, other(ingress)]) {
        const {status, body} = await read(am, item.path, [200]);
        check(status === 200 && body?.sha256 === item.sha256 && body.path === item.path.slice('/home/'.length)
          && body.content_base64 === file.bytes.toString('base64'),
          'browser_attachments_runtime_bytes_changed');
      }
    }
    const submitted = prompt + markerPrefix + JSON.stringify(kept.map(item => ({
      name: item.name, path: item.path, media_type: item.media_type, size_bytes: item.size_bytes,
    })), null, 2);
    const answer = 'WS4 complete ' + marker;
    await adapter.prepareTurn({caseId, marker});
    await withRestoration(async () => {
      await adapter.releaseTurn({caseId});
      const [response] = await Promise.all([
        page.waitForResponse(reply => new URL(reply.url()).origin === origin
          && isPreparedSend(reply.request(), agentId, submitted), {timeout}),
        send.click({timeout}),
      ]);
      check(response.ok(), 'browser_attachments_send_refused');
      await page.getByText(answer, {exact: true}).waitFor({timeout: 90000});
      const sequences = [];
      for (const am of [ingress, other(ingress)]) {
        sequences.push(await poll(async () => {
          const {body} = await api.rawRequest(am, 'GET', '/api/v1' + agent + '/transcript');
          check(body?.agent_id === agentId && Array.isArray(body.turns), 'browser_attachments_transcript_unbound');
          const matches = body.turns.filter(turn => turn.owner_message === submitted);
          check(matches.length <= 1, 'browser_attachments_turn_duplicated');
          return matches[0]?.outcome?.type === 'completed' ? matches[0].turn_sequence : null;
        }, Number.isSafeInteger, {timeout, code: 'browser_attachments_turn_missing'}));
      }
      check(sequences[0] === sequences[1], 'browser_attachments_turn_unbound');
      const oracle = await poll(() => adapter.oracle({caseId}), value => value.active_streams === 0,
        {timeout, code: 'browser_attachments_provider_still_active'});
      assertOracle(oracle, caseId, 1);
    }, () => adapter.releaseTurn({caseId}), recordFailure);
  }, async () => {
    releaseUpload();
    try {
      await page.unroute(chooserRoute, loseReply);
      await interruptedUpload;
      for (const item of receipts.values()) {
        const relative = item.path.slice('/home/'.length);
        const {status, body} = await api.rawRequest(other(ingress), 'DELETE',
          files + '?path=' + encodeURIComponent(relative) + '&recursive=false', {allowed: [200, 404]});
        check(status === 200 ? body?.path === relative && body.deleted === true : body?.code === 'file_not_found',
          'browser_attachments_cleanup_unbound');
        for (const am of [ingress, other(ingress)]) {
          const current = await read(am, item.path, [404]);
          check(current.status === 404 && current.body?.code === 'file_not_found', 'browser_attachments_cleanup_incomplete');
        }
      }
      check(seen.length <= 4, 'browser_attachments_unexpected_upload_dispatched');
    } finally {
      page.off('request', observe);
    }
  }, recordFailure);
}
