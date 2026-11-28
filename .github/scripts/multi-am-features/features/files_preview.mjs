import {randomUUID} from 'node:crypto';
import {assertResponse, check, hash, id, poll, terminal} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

const previewBytes = 1024 * 1024;
const timeout = 15000;
const agentPath = agentId => '/api/v1/agents/' + agentId;

export async function withChangedFile({api, adapter, record, recordFailure}, run) {
  const {agentId} = record;
  check(id(agentId), 'file_preview_agent_required');
  const root = agentPath(agentId);
  const owners = ['a', 'b'];
  const details = await Promise.all(owners.map(am => api.rawRequest(am, 'GET', root)));
  const detail = details[0].body;
  const directory = detail?.runtime_metadata?.working_directory ?? detail?.worktree?.path;
  check(typeof directory === 'string' && directory.startsWith('/home/')
    && directory.slice(6).split('/').every(part => part && part !== '.' && part !== '..')
    && details.every(({body}) => body?.id === agentId
      && (body.runtime_metadata?.working_directory ?? body.worktree?.path) === directory),
  'file_preview_runtime_path_unbound');

  const suffix = randomUUID().replaceAll('-', '');
  const marker = 'WS4_FILE_' + suffix;
  const caseId = 'file-preview-' + suffix;
  const input = 'Create the disposable fixture file for ' + marker + '.';
  const answer = 'WS4 complete ' + marker;
  const filename = 'cma-preview-' + suffix + '.txt';
  const largeFilename = 'cma-preview-' + suffix + '-large.txt';
  const file = directory.slice(6) + '/' + filename;
  const largeFile = directory.slice(6) + '/' + largeFilename;
  const text = marker + '\n';
  const files = root + '/files';
  const content = target => files + '/content?' + new URLSearchParams({path: target, max_bytes: previewBytes});
  const absent = async (am, target) => {
    const {body} = await api.rawRequest(am, 'GET', content(target), {allowed: [404], discardSuccessfulReply: true});
    check(body?.code === 'file_not_found', 'file_preview_fixture_absence_unconfirmed');
  };
  for (const am of owners) for (const target of [file, largeFile]) await absent(am, target);

  let stream;
  await withRestoration(async () => {
    await adapter.prepareTurn({caseId, marker});
    await adapter.releaseTurn({caseId});
    stream = await api.send('a', agentId, input, caseId);
    const response = await poll(async () => {
      const {body} = await api.rawRequest('b', 'GET', '/api/v2/responses/' + stream.responseId);
      return assertResponse(body, agentId, stream.responseId);
    }, terminal, {timeout: 90000, code: 'file_preview_turn_timeout'});
    await stream.done;
    check(stream.error === null && response.status === 'completed'
      && JSON.stringify(response.output ?? []).includes(answer), 'file_preview_turn_result_unbound');
    const witness = await adapter.oracle({caseId});
    check(witness?.case_id === caseId && witness.file_preview_calls === 1 && witness.file_preview_outputs === 1
      && witness.request_count === 2 && witness.completed_effect_count === 2 && witness.active_streams === 0,
    'file_preview_edit_effect_unbound');

    const oversize = Buffer.alloc(previewBytes + 1, 'a');
    const {body: created} = await api.request('b', 'POST', files, {direct: true,
      body: {path: largeFile, content_base64: oversize.toString('base64')}});
    check(created?.path === largeFile && created.size_bytes === oversize.length && created.sha256 === hash(oversize.toString('ascii')),
      'file_preview_limit_fixture_unbound');
    for (const am of owners) {
      const {body} = await api.rawRequest(am, 'GET', content(file));
      check(body?.path === file && body.sha256 === hash(text) && body.size_bytes === Buffer.byteLength(text)
        && body.content_base64 === Buffer.from(text).toString('base64'), 'file_preview_agent_edit_bytes_changed');
      const refused = await api.rawRequest(am, 'GET', content(largeFile), {allowed: [413], discardSuccessfulReply: true});
      check(refused.body?.code === 'file_too_large', 'file_preview_limit_refusal_untyped');
    }
    await run({agentId, file, filename, largeFile, largeFilename, text, input, answer});
  }, async () => {
    if (stream) { stream.abort(); await stream.done; }
    const cleanup = await Promise.allSettled([file, largeFile].map(async target => {
      const {status, body} = await api.request('b', 'DELETE', files + '?' + new URLSearchParams({path: target}),
        {direct: true, allowed: [200, 404]});
      check(status === 404 ? body?.code === 'file_not_found' : body?.path === target && body.deleted === true,
        'file_preview_cleanup_unconfirmed');
      for (const am of owners) await absent(am, target);
    }));
    const failures = cleanup.filter(result => result.status === 'rejected').map(result => result.reason);
    if (failures.length) throw new AggregateError(failures, 'file_preview_cleanup_failed');
  }, recordFailure);
}

export async function browserChangedFilePreview(page, {agentId, file, filename, largeFile, largeFilename, text}) {
  check(new URL(page.url()).pathname === '/agents/' + agentId, 'file_preview_agent_navigation_unbound');
  const origin = new URL(page.url()).origin;
  const contentPath = agentPath(agentId) + '/files/content';
  const isContent = (url, target) => url.origin === origin && url.pathname === contentPath
    && url.searchParams.get('path') === target;
  await page.getByRole('button', {name: 'Show Agent computer', exact: true}).click({timeout});
  await page.getByRole('tab', {name: 'Files', exact: true}).click({timeout});
  const panel = page.getByRole('tabpanel', {name: 'Files', exact: true});
  const changes = panel.getByRole('region', {name: 'Changed files', exact: true});
  const changed = changes.getByRole('button', {name: 'Open changed file ' + file, exact: true});
  const [response] = await Promise.all([
    page.waitForResponse(reply => reply.request().method() === 'GET' && isContent(new URL(reply.url()), file),
      {timeout}),
    changed.click({timeout}),
  ]);
  const body = await response.json();
  check(response.status() === 200 && new URL(response.url()).searchParams.get('max_bytes') === String(previewBytes)
    && body?.path === file && body.sha256 === hash(text) && body.content_base64 === Buffer.from(text).toString('base64'),
  'file_preview_browser_read_unbound');
  const selected = panel.getByLabel('Selected file', {exact: true});
  await selected.getByText('/home/' + file, {exact: true}).waitFor({timeout});
  const preview = panel.getByRole('textbox', {name: 'Contents of ' + filename, exact: true});
  await poll(() => preview.inputValue({timeout}), value => value === text,
    {timeout, interval: 100, code: 'file_preview_browser_bytes_changed'});

  const requests = [];
  const observe = request => {
    if (isContent(new URL(request.url()), largeFile)) requests.push(request);
  };
  page.on('request', observe);
  try {
    await panel.getByRole('button', {name: largeFilename, exact: true}).click({timeout});
    await selected.getByText('/home/' + largeFile, {exact: true}).waitFor({timeout});
    await panel.getByText('Preview unavailable. Download this file to view its contents.', {exact: true}).waitFor({timeout});
    check(await preview.count() === 0 && requests.length === 0, 'file_preview_browser_loaded_oversize');
  } finally {
    page.off('request', observe);
  }
}
