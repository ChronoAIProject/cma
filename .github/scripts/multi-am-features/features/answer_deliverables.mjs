import {randomUUID} from 'node:crypto';
import {readFile} from 'node:fs/promises';
import {isDeepStrictEqual as equal} from 'node:util';
import {assertOracle, check, hash, id, other, poll} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';
import {documentIdentity} from './document_identity.mjs';
import {removeFixtureDirectory} from './files_browser.mjs';

const timeout = 20000;

export async function answerDeliverables({page, origin, api, outsider, adapter, ingress, ownerId, record, recordFailure}) {
  const {agentId, sandboxId, operationId} = record;
  check([agentId, sandboxId, operationId, ownerId].every(id), 'deliverable_fixture_lineage_required');
  const peer = other(ingress);
  const ams = [ingress, peer];
  const agent = '/api/v1/agents/' + agentId;
  const files = agent + '/files';
  const documents = '/api/v1/documents';
  const suffix = randomUUID();
  const directory = 'ci-browser-' + suffix;
  const reportName = 'report-' + suffix + '.txt';
  const report = directory + '/' + reportName;
  const large = directory + '/oversize.txt';
  const largeText = 'x'.repeat(1024 * 1024 + 1);
  const reportText = 'Answer deliverable ' + suffix + '\n';
  const documentTitle = 'deliverable-' + suffix;
  const source = '# Original deliverable ' + suffix + '\n';
  const updatedHeading = 'Edited deliverable ' + suffix;
  const updatedSource = '# ' + updatedHeading + '\n\nNew draft.\n';
  const createKey = randomUUID();
  const documentId = documentIdentity('doc_', [ownerId, createKey]);
  const documentPath = documents + '/' + documentId;
  const marker = 'WS4_DELIVERY_' + suffix.replaceAll('-', '');
  const prompt = 'Return the fixture deliverables for ' + marker + '.';
  const answer = 'WS4 complete ' + marker;
  const caseId = 'browser-deliverables-' + suffix;
  const request = (am, method, path, options) => api.request(am, method, path, {direct: true, ...options});
  const write = (path, text, expected) => request(peer, 'POST', files, {body: {path,
    content_base64: Buffer.from(text).toString('base64'), expected_sha256: expected}});
  const removeFile = async target => {
    const {body} = await request(peer, 'DELETE', files + '?' + new URLSearchParams({path: target, recursive: 'false'}));
    check(body?.path === target && body.deleted === true, 'deliverable_fixture_file_remove_unbound');
  };

  const browserDownloads = new Set();
  const browserDownloadPath = request => {
    const url = new URL(request.url());
    if (request.method() === 'GET' && url.origin === origin
        && url.pathname.startsWith(agent + '/answers/') && url.pathname.endsWith('/download')) {
      return decodeURIComponent(url.pathname);
    }
    return null;
  };
  const observeDownload = request => {
    const path = browserDownloadPath(request);
    if (path) browserDownloads.add(path);
  };
  let folderAttempted, documentAttempted, folder, document, stream, largeDownload;
  page.on('request', observeDownload);
  await withRestoration(async () => {
    // Each cleanup target belongs to this journey and is installed only once.
    folderAttempted = true;
    folder = (await request(peer, 'POST', files, {body: {path: directory, directory: true, content_base64: ''}})).body;
    check(folder?.path === directory, 'deliverable_fixture_folder_unbound');
    const live = (await write(report, reportText)).body;
    check(live?.path === report && live.size_bytes === Buffer.byteLength(reportText)
      && live.sha256 === hash(reportText), 'deliverable_fixture_file_unbound');
    const oversized = (await write(large, largeText)).body;
    check(oversized?.path === large && oversized.size_bytes === Buffer.byteLength(largeText)
      && oversized.sha256 === hash(largeText), 'deliverable_fixture_large_file_unbound');
    documentAttempted = true;
    document = (await request(ingress, 'POST', documents, {
      key: createKey, allowed: [201], body: {title: documentTitle, markdown: source},
    })).body;
    check(document?.id === documentId && document.title === documentTitle && document.owner_subject === ownerId
      && document.sha256 === hash(source) && Number.isSafeInteger(document.source_revision)
      && Number.isSafeInteger(document.revision),
    'deliverable_fixture_document_unbound');
    const resources = [{kind: 'live_file', path: report}, {kind: 'live_file', path: large},
      {kind: 'document', id: document.id, revision: document.source_revision}];
    const descriptions = [];
    for (const [index, resource] of resources.entries()) {
      const {body} = await request(peer, 'POST', agent + '/deliverable-references', {body: resource});
      check(body?.reference?.agent_id === agentId && equal(body.reference.resource, resource)
        && body.reference.sha256 === [live.sha256, oversized.sha256, document.sha256][index]
        && typeof body.marker === 'string', 'deliverable_fixture_description_unbound');
      descriptions.push(body);
    }
    const markers = descriptions.map(value => value.marker);
    const exactAnswer = answer + '\n\n' + markers.join('\n');
    await adapter.prepareTurn({caseId, marker, deliverables: markers});
    await adapter.releaseTurn({caseId});
    stream = await api.send(ingress, agentId, prompt, caseId);
    await stream.done;
    check(stream.error === null, 'deliverable_fixture_send_failed');
    const oracle = await poll(() => adapter.oracle({caseId}), value => value.active_streams === 0,
      {timeout, code: 'deliverable_fixture_provider_active'});
    assertOracle(oracle, caseId, 1);

    const observations = [];
    for (const am of ams) {
      observations.push(await poll(async () => {
        const {body} = await request(am, 'GET', agent + '/transcript');
        check(body?.agent_id === agentId && Array.isArray(body.turns), 'deliverable_fixture_transcript_unbound');
        const turns = body.turns.filter(turn => turn.owner_message === prompt);
        check(turns.length <= 1, 'deliverable_fixture_duplicate_turn');
        return turns[0]?.outcome?.type === 'completed' ? turns[0] : null;
      }, Boolean, {timeout, code: 'deliverable_fixture_turn_missing'}));
    }
    const [turn] = observations;
    check(equal(...observations), 'deliverable_fixture_peer_turn_changed');
    const replies = turn.messages.filter(message => message.role === 'assistant' && message.content === exactAnswer);
    check(replies.length === 1 && Number.isSafeInteger(replies[0].sequence), 'deliverable_fixture_answer_unbound');
    const answerId = 'event:' + replies[0].sequence;
    const listPath = agent + '/answers/' + encodeURIComponent(answerId) + '/deliverables';
    let items;
    for (const am of ams) {
      const {body} = await request(am, 'GET', listPath);
      check(body?.agent_id === agentId && body.answer_id === answerId && body.turn_sequence === turn.turn_sequence
        && body.items?.length === descriptions.length && descriptions.every(({reference}) =>
          body.items.filter(item => equal(item.reference, reference) && /^delivery_[a-f0-9]{64}$/.test(item.id)).length === 1),
      'deliverable_fixture_list_unbound');
      if (items) check(equal(items, body.items), 'deliverable_fixture_peer_list_changed');
      items = body.items;
      const refused = await outsider.rawRequest(am, 'GET', listPath, {allowed: [403, 404]});
      check(['resource_forbidden', 'resource_not_found', 'agent_not_found', 'deliverable_not_found'].includes(refused.body?.code)
        && !Object.hasOwn(refused.body, 'items'),
        'deliverable_fixture_outsider_list_untyped');
    }
    const [reportItem, largeItem, documentItem] = descriptions.map(({reference}) =>
      items.find(item => equal(item.reference, reference)));
    const downloadPath = item => listPath + '/' + item.id + '/download';
    largeDownload = decodeURIComponent(downloadPath(largeItem));
    for (const am of ams) {
      const denied = await outsider.rawRequest(am, 'GET', downloadPath(reportItem),
        {allowed: [403, 404], discardSuccessfulReply: true});
      check(['resource_forbidden', 'resource_not_found', 'agent_not_found', 'deliverable_not_found'].includes(denied.body?.code),
        'deliverable_fixture_outsider_download_untyped');
    }
    await page.goto('/agents/' + agentId, {waitUntil: 'domcontentloaded', timeout});
    await page.getByText(answer, {exact: true}).waitFor({timeout});
    const list = page.getByRole('list', {name: 'Answer deliverables', exact: true})
      .filter({has: page.getByRole('button', {name: 'Preview ' + reportName, exact: true})});
    const row = item => list.getByRole('listitem')
      .filter({has: page.getByRole('button', {name: 'Preview ' + item.reference.name, exact: true})});
    const preview = page.getByRole('complementary', {name: 'Deliverable preview', exact: true});
    const open = async item => {
      await row(item).getByRole('button', {name: 'Preview ' + item.reference.name, exact: true}).click({timeout});
      await preview.getByRole('heading', {name: item.reference.name, exact: true}).waitFor({timeout});
    };
    const close = () => preview.getByRole('button', {name: 'Close preview', exact: true}).click({timeout});
    const download = async (item, bytes) => {
      const target = decodeURIComponent(downloadPath(item));
      const [saved, response] = await Promise.all([page.waitForEvent('download', {timeout}),
        page.waitForResponse(reply => browserDownloadPath(reply.request()) === target, {timeout}),
        preview.getByRole('button', {name: 'Download ' + item.reference.name, exact: true}).click({timeout})]);
      try {
        check(response.status() === 200 && saved.suggestedFilename() === item.reference.name
          && (await readFile(await saved.path())).equals(Buffer.from(bytes)),
          'deliverable_fixture_download_changed');
      } finally {
        await saved.delete();
      }
    };
    const verify = async (item, status) => {
      for (const am of ams) {
        const {body} = await request(am, 'GET', listPath + '/' + item.id + '?verify=true');
        check(body?.agent_id === agentId && body.answer_id === answerId && body.turn_sequence === turn.turn_sequence
          && body.item?.id === item.id && equal(body.item.reference, item.reference) && body.item.status === status,
        'deliverable_fixture_status_changed');
      }
    };

    await open(reportItem);
    const content = preview.getByRole('textbox', {name: 'Contents of ' + reportName, exact: true});
    await poll(() => content.inputValue({timeout}), value => value === reportText,
      {timeout, code: 'deliverable_fixture_file_preview_changed'});
    await download(reportItem, reportText);
    await close();
    await write(report, 'Changed fixture file.\n', live.sha256);
    await verify(reportItem, 'changed');
    await open(reportItem);
    await preview.getByText('File changed since this answer', {exact: true}).waitFor({timeout});
    await close();
    await removeFile(report);
    await verify(reportItem, 'missing');
    await open(reportItem);
    await preview.getByText('File deleted', {exact: true}).waitFor({timeout});
    await close();
    await open(largeItem);
    await preview.getByText('Preview exceeds the 1 MiB limit. Download to view this file.', {exact: true}).waitFor({timeout});
    await close();

    const updated = (await request(peer, 'PATCH', documentPath, {
      key: randomUUID(), body: {expected_revision: document.revision, markdown: updatedSource},
    })).body;
    check(updated?.id === document.id && updated.revision === document.revision + 1
      && updated.source_revision > document.source_revision && updated.revision === updated.source_revision
      && updated.sha256 === hash(updatedSource), 'deliverable_fixture_document_update_unbound');
    for (const am of ams) {
      const {body} = await request(am, 'GET', documentPath + '/source');
      check(body?.id === document.id && body.revision === updated.source_revision
        && body.markdown === updatedSource, 'deliverable_fixture_document_current_source_changed');
    }
    await verify(documentItem, 'version_changed');
    await open(documentItem);
    const originalHeading = preview.getByText('Original deliverable ' + suffix, {exact: true});
    const latestHeading = preview.getByText(updatedHeading, {exact: true});
    const previewFailure = preview.getByText(/^(Preview failed; retry\.|File changed since this answer)$/);
    await poll(async () => {
      check(!await previewFailure.isVisible() && !await latestHeading.isVisible(),
        'deliverable_fixture_document_preview_changed');
      return originalHeading.isVisible();
    }, Boolean, {timeout, code: 'deliverable_fixture_document_preview_missing'});
    await preview.getByText('Document has a newer revision; download keeps this revision', {exact: true}).waitFor({timeout});
    await download(documentItem, source);
    await close();
  }, async () => {
    if (stream) { stream.abort(); await stream.done; }
    await withRestoration(async () => {
      if (folderAttempted) await removeFixtureDirectory({api, path: files, ingress, directory, acknowledged: folder?.path === directory});
    }, async () => {
      if (!documentAttempted) return;
      const current = await request(peer, 'GET', documentPath, {allowed: [200, 404]});
      if (current.status === 200) {
        check(current.body?.id === documentId && current.body.title === documentTitle
          && current.body.owner_subject === ownerId && [hash(source), hash(updatedSource)].includes(current.body.sha256),
        'deliverable_fixture_document_cleanup_foreign');
        await request(peer, 'DELETE', documentPath, {key: randomUUID(), body: {expected_revision: current.body.revision}});
      }
      for (const am of ams) {
        const {body} = await request(am, 'GET', documentPath, {allowed: [404]});
        check(body?.code === 'document_not_found', 'deliverable_fixture_document_cleanup_unbound');
      }
    }, recordFailure);
  }, recordFailure).finally(() => page.off('request', observeDownload));
  check(!browserDownloads.has(largeDownload), 'deliverable_fixture_oversize_preview_fetched');
}
