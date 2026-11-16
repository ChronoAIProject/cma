import {randomUUID, createHash} from 'node:crypto';
import {check} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

const hash = text => createHash('sha256').update(text).digest('hex');
const encoded = text => Buffer.from(text).toString('base64');

// Reuses the run17-created Agent, native home, provider and real authentication.
// Every request on this API object is routed using the current live owner.
export async function userCases({record: resource, api, runCase, recordFailure}) {
  const base = `/api/v1/agents/${resource.agentId}`;
  await runCase('user.files_roundtrip', async () => {
    const directory = `ci-${randomUUID()}`;
    const file = directory + '/message.txt';
    const text = 'two AM file ' + randomUUID() + '\n';
    const read = async () => (await api.request('a', 'GET', base + '/files/content?path=' + encodeURIComponent(file))).body;
    let createdDirectory = false;
    try {
      const folder = await api.request('a', 'POST', base + '/files', {
        body: {path: directory, content_base64: '', directory: true}});
      check(folder.body.path === directory, 'directory_create_identity'); createdDirectory = true;
      const uploaded = await api.request('a', 'POST', base + '/files', {
        body: {path: file, content_base64: encoded(text)}});
      check(uploaded.body.sha256 === hash(text), 'file_create_digest');
      const before = await read();
      check(before.path === file && before.sha256 === hash(text)
        && Buffer.from(before.content_base64, 'base64').toString() === text, 'file_readback');
      const listing = (await api.request('a', 'GET', base + '/files/tree?path=' + encodeURIComponent(directory))).body;
      check(listing.entries.some(entry => entry.name === 'message.txt' && entry.kind === 'file'), 'file_tree_entry');
      const updated = text + 'updated\n';
      await api.request('a', 'POST', base + '/files', {body: {
        path: file, expected_sha256: before.sha256, content_base64: encoded(updated)}});
      await api.request('a', 'POST', base + '/files', {allowed: [409], body: {
        path: file, expected_sha256: before.sha256, content_base64: encoded('stale overwrite')}});
      check((await read()).sha256 === hash(updated), 'stale_file_write_changed_content');
      await api.router.dispatch(resource.agentId, async am => {
        const response = await fetch(api.url(am, base + '/files/download?path=' + encodeURIComponent(file)), {
          headers: api.headers(), redirect: 'error', signal: AbortSignal.timeout(30000)});
        check(response.status === 200 && await response.text() === updated, 'file_download_bytes');
      });
    } finally {
      if (createdDirectory) {
        const deleted = await api.request('a', 'DELETE', base + '/files?recursive=true&path=' + encodeURIComponent(directory));
        check(deleted.body.deleted === true, 'file_cleanup_ack');
        const listing = (await api.request('a', 'GET', base + '/files/tree')).body;
        check(!listing.entries.some(entry => entry.name === directory), 'file_cleanup_not_visible');
      }
    }
  });
  await runCase('user.rename_cas', async () => {
    const before = (await api.request('a', 'GET', base + '/preferences')).body;
    check(Number.isSafeInteger(before.revision), 'preference_revision_missing');
    const title = 'CI ' + randomUUID();
    await withRestoration(async () => {
      await api.request('a', 'PATCH', base + '/preferences', {
        body: {expected_revision: before.revision, title}, key: randomUUID()});
      const visible = (await api.request('a', 'GET', base)).body;
      check(visible.id === resource.agentId && (visible.owner_title === title || visible.title === title), 'rename_readback');
      await api.request('a', 'PATCH', base + '/preferences', {allowed: [409],
        body: {expected_revision: before.revision, title: resource.name}, key: randomUUID()});
    }, async () => {
      const current = (await api.request('a', 'GET', base + '/preferences')).body;
      if (current.title !== before.title) {
        check(current.title === title, 'rename_restore_foreign_title');
        await api.request('a', 'PATCH', base + '/preferences', {
          body: {expected_revision: current.revision, title: before.title ?? resource.name}, key: randomUUID()});
      }
      const restored = (await api.request('a', 'GET', base)).body;
      check([restored.title, restored.owner_title].includes(before.title ?? resource.name), 'rename_restore_missing');
    }, recordFailure);
  });
}
