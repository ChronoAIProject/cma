import { createHash, webcrypto } from 'node:crypto';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { createClient } from '@cma/client';
import { PublishArtifact } from './PublishArtifact';

beforeEach(() => vi.stubGlobal('crypto', webcrypto));
afterEach(() => vi.unstubAllGlobals());
const contents = new TextEncoder().encode('review');
const metadata = {
  id: `art_${'a'.repeat(64)}`,
  agent_id: 'agt_1',
  name: 'Review.txt',
  media_type: 'text/plain',
  size_bytes: 6,
  sha256: createHash('sha256').update(contents).digest('hex'),
  created_at: '2026-09-01T00:00:00Z',
};
function fixture(bytes = contents, firstResponse: 'lost' | 'wrong-hash' = 'lost') {
  const requests: { body: unknown; key: string | null }[] = [];
  const entries = new Map<string, { body: unknown; receipt: typeof metadata }>();
  const receipt = {
    ...metadata,
    size_bytes: bytes.length,
    sha256: createHash('sha256').update(bytes).digest('hex'),
  };
  const state = { allowed: true };
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input, init) => {
      if (String(input).includes('/resource-access/'))
        return Response.json({
          capabilities: state.allowed ? ['manage_sharing'] : [],
          revalidate_at: null,
          policy_revision: 1,
        });
      expect(init?.method).toBe('POST');
      const body: unknown = JSON.parse(String(init?.body));
      const key = new Headers(init?.headers).get('idempotency-key');
      if (key === null) throw new Error('Publication is missing its idempotency key');
      requests.push({ body, key });
      const entry = entries.get(key);
      if (entry === undefined) entries.set(key, { body, receipt });
      else expect(body).toEqual(entry.body);
      if (requests.length === 1) {
        if (firstResponse === 'lost') throw new Error('Lost response');
        const wrongHash = createHash('sha256').update(Buffer.alloc(bytes.length, 65)).digest('hex');
        return Response.json({ ...receipt, sha256: wrongHash }, { status: 201 });
      }
      return Response.json(entry?.receipt ?? receipt, { status: 201 });
    },
  });
  const published = vi.fn();
  const access = vi.fn();
  const view = render(
    <PublishArtifact client={client} id="agt_1" onPublished={published} onAccessChanged={access} />,
  );
  const file = new File([bytes], 'Review.txt', { type: 'text/plain' });
  Object.defineProperty(file, 'arrayBuffer', {
    value: async () => bytes.buffer,
  });
  return { requests, entries, state, published, access, file, view };
}
it('requires explicit content approval and retries an uncertain publication with the original body/key', async () => {
  const test = fixture();
  const user = userEvent.setup();
  await user.upload(screen.getByLabelText('Artifact file'), test.file);
  expect(screen.getByRole('button', { name: 'Publish artifact' })).toBeDisabled();
  await user.click(screen.getByRole('checkbox'));
  await user.click(screen.getByRole('button', { name: 'Publish artifact' }));
  const retry = await screen.findByRole('button', { name: 'Retry artifact publication' });
  await waitFor(() => expect(retry).toBeEnabled());
  expect(test.requests).toHaveLength(1);
  expect(screen.getByLabelText('Artifact file')).toBeDisabled();
  await user.click(retry);
  await screen.findByText('Artifact published.');
  expect(test.requests).toHaveLength(2);
  expect(test.requests[1]).toEqual(test.requests[0]);
  expect(test.requests[0]?.body).toEqual({
    name: 'Review.txt',
    media_type: 'text/plain',
    content_base64: 'cmV2aWV3',
  });
  expect(test.published).toHaveBeenCalledOnce();
});

it('keeps a 1 MiB artifact unconfirmed after a mismatched SHA-256 until the owner replays it', async () => {
  const bytes = new Uint8Array(1024 * 1024);
  const test = fixture(bytes, 'wrong-hash');
  const user = userEvent.setup();
  await user.upload(screen.getByLabelText('Artifact file'), test.file);
  await user.click(screen.getByRole('checkbox'));
  await user.click(screen.getByRole('button', { name: 'Publish artifact' }));

  const retry = await screen.findByRole('button', { name: 'Retry artifact publication' });
  await waitFor(() => expect(retry).toBeEnabled());
  expect(screen.getByRole('status')).toHaveTextContent(/receipt did not match/i);
  expect(test.requests).toHaveLength(1);
  expect(test.published).not.toHaveBeenCalled();
  const submitted = test.requests[0]?.body as { content_base64: string };
  expect(Buffer.from(submitted.content_base64, 'base64').equals(Buffer.from(bytes))).toBe(true);

  await user.click(retry);
  await screen.findByText('Artifact published.');
  expect(test.requests).toHaveLength(2);
  expect(test.requests[1]).toEqual(test.requests[0]);
  expect(test.entries.size).toBe(1);
  expect(test.published).toHaveBeenCalledOnce();
});

it('rejects a file one byte above 1 MiB without publishing it', async () => {
  const test = fixture(new Uint8Array(1024 * 1024 + 1));
  const user = userEvent.setup();
  await user.upload(screen.getByLabelText('Artifact file'), test.file);
  await user.click(screen.getByRole('checkbox'));
  await user.click(screen.getByRole('button', { name: 'Publish artifact' }));
  expect(await screen.findByRole('status')).toHaveTextContent(
    'Choose a file between 1 byte and 1 MiB.',
  );
  expect(test.requests).toHaveLength(0);
  expect(test.published).not.toHaveBeenCalled();
});
it('rechecks manager authority after file selection and refuses publication after removal', async () => {
  const test = fixture();
  test.state.allowed = false;
  const user = userEvent.setup();
  await user.upload(screen.getByLabelText('Artifact file'), test.file);
  await user.click(screen.getByRole('checkbox'));
  await user.click(screen.getByRole('button', { name: 'Publish artifact' }));
  await screen.findByText('Current access does not allow publishing artifacts.');
  expect(test.requests).toHaveLength(0);
  expect(test.access).toHaveBeenCalledOnce();
});

it('discards retained approval and request bytes when the SDK client changes for the same Agent', async () => {
  const test = fixture();
  const user = userEvent.setup();
  await user.upload(screen.getByLabelText('Artifact file'), test.file);
  await user.click(screen.getByRole('checkbox'));
  await user.click(screen.getByRole('button', { name: 'Publish artifact' }));
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Retry artifact publication' })).toBeEnabled(),
  );
  const fetch = vi.fn();
  const client = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  test.view.rerender(
    <PublishArtifact
      client={client}
      id="agt_1"
      onPublished={test.published}
      onAccessChanged={test.access}
    />,
  );
  expect(screen.getByRole('button', { name: 'Publish artifact' })).toBeDisabled();
  expect(screen.getByRole('checkbox')).not.toBeChecked();
  expect(screen.getByLabelText('Artifact file')).toBeEnabled();
  expect(fetch).not.toHaveBeenCalled();
});
