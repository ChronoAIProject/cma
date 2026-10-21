import { afterEach, describe, expect, it, vi } from 'vitest';
import { agentFilesClient } from './agentFiles';

const digest = 'a'.repeat(64);
afterEach(() => vi.unstubAllGlobals());
function response(value: unknown) {
  vi.stubGlobal(
    'fetch',
    vi.fn(
      async () =>
        new Response(JSON.stringify(value), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        }),
    ),
  );
}
describe('Agent filesystem public receipts', () => {
  it('rejects a preview whose byte count differs from its content', async () => {
    response({ path: 'note.txt', size_bytes: 99, sha256: digest, content_base64: 'aGk=' });
    await expect(agentFilesClient().read('agent', 'note.txt')).rejects.toThrow(
      'invalid file preview',
    );
  });
  it('rejects malformed base64 and cross-file responses', async () => {
    response({ path: 'note.txt', size_bytes: 2, sha256: digest, content_base64: 'a=k=' });
    await expect(agentFilesClient().read('agent', 'note.txt')).rejects.toThrow(
      'invalid file preview',
    );
    response({ path: 'other.txt', size_bytes: 2, sha256: digest, content_base64: 'aGk=' });
    await expect(agentFilesClient().read('agent', 'note.txt')).rejects.toThrow(
      'invalid file preview',
    );
  });
  it('rejects listings that claim a descendant as an immediate child', async () => {
    response({
      path: '',
      entries: [{ path: 'child/note', name: 'note', kind: 'file', size_bytes: 1, hidden: false }],
    });
    await expect(agentFilesClient().list('agent', '')).rejects.toThrow('invalid directory listing');
  });
  it('accepts a bounded save only when the exact file and byte count are confirmed', async () => {
    response({ path: 'note', size_bytes: 2, sha256: digest, deleted: null });
    await expect(
      agentFilesClient().write('agent', { path: 'note', content_base64: 'aGk=' }),
    ).resolves.toMatchObject({ path: 'note', size_bytes: 2 });
    response({ path: 'note', size_bytes: 1, sha256: digest });
    await expect(
      agentFilesClient().write('agent', { path: 'note', content_base64: 'aGk=' }),
    ).rejects.toThrow('did not confirm');
  });
  it('never claims successful deletion for a different file or a negative receipt', async () => {
    response({ path: 'other', deleted: true });
    await expect(agentFilesClient().remove('agent', 'note', false)).rejects.toThrow(
      'did not confirm',
    );
    response({ path: 'note', deleted: false });
    await expect(agentFilesClient().remove('agent', 'note', false)).rejects.toThrow(
      'did not confirm',
    );
  });
});
