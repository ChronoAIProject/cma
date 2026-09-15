import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { PublishArtifact } from './PublishArtifact';

afterEach(() => vi.restoreAllMocks());
const text = 'review';
const metadata = {
  id: `art_${'a'.repeat(64)}`,
  agent_id: 'agt_1',
  name: 'Review.txt',
  media_type: 'text/plain',
  size_bytes: 6,
  sha256: 'b'.repeat(64),
  created_at: '2026-09-01T00:00:00Z',
};
function fixture() {
  const requests: { body: unknown; key: string | null }[] = [];
  const state = { lose: true, allowed: true };
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
      requests.push({
        body: JSON.parse(String(init?.body)),
        key: new Headers(init?.headers).get('idempotency-key'),
      });
      if (state.lose) {
        state.lose = false;
        throw new Error('Lost response');
      }
      return Response.json(metadata, { status: 201 });
    },
  });
  const published = vi.fn();
  const access = vi.fn();
  const view = render(
    <PublishArtifact client={client} id="agt_1" onPublished={published} onAccessChanged={access} />,
  );
  const file = new File([text], 'Review.txt', { type: 'text/plain' });
  Object.defineProperty(file, 'arrayBuffer', {
    value: async () => new TextEncoder().encode(text).buffer,
  });
  return { client, requests, state, published, access, file, view };
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
