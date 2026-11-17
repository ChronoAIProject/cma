import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createHash, webcrypto } from 'node:crypto';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient } from '@cma/client';
import { AgentArtifacts, authorizedArtifacts } from './AgentArtifacts';
import { ArtifactButton } from './ArtifactButton';

const bytes = new TextEncoder().encode('review');
const id = `art_${'a'.repeat(64)}`;
const metadata = {
  id,
  agent_id: 'agt_1',
  name: 'Review.txt',
  media_type: 'text/plain',
  size_bytes: bytes.length,
  sha256: createHash('sha256').update(bytes).digest('hex'),
  created_at: '2026-09-01T00:00:00Z',
};
afterEach(() => vi.unstubAllGlobals());
function fixture() {
  const state = { revoked: false, policy: 1, manager: false, corrupt: false, second: false };
  const paths: string[] = [];
  const fetch = vi.fn(async (input: RequestInfo | URL) => {
    const url = new URL(String(input));
    paths.push(url.pathname + url.search);
    if (state.revoked)
      return Response.json(
        { code: 'resource_not_found', detail: 'Access removed' },
        { status: 404 },
      );
    if (url.pathname.includes('/resource-access/'))
      return Response.json({
        capabilities: ['view_artifacts', ...(state.manager ? ['manage_sharing'] : [])],
        revalidate_at: null,
        policy_revision: state.policy,
      });
    if (url.pathname.endsWith('/download'))
      return new Response(state.corrupt ? 'changed' : bytes, {
        headers: { 'content-type': 'application/octet-stream' },
      });
    if (url.pathname.endsWith('/artifacts'))
      return Response.json(
        url.searchParams.has('cursor')
          ? { items: [] }
          : { items: [metadata], ...(state.second ? { next: id } : {}) },
      );
    throw new Error(`Unexpected artifact fetch ${url.pathname}`);
  });
  return {
    state,
    paths,
    fetch,
    client: createClient({ baseUrl: 'https://fixture.invalid', fetch }),
  };
}
function downloads() {
  vi.stubGlobal('crypto', webcrypto);
  const create = vi.fn(() => 'blob:fixture');
  const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
  const original = URL;
  class DownloadURL extends original {
    static createObjectURL = create;
    static revokeObjectURL = vi.fn();
  }
  vi.stubGlobal('URL', DownloadURL);
  return { create, click };
}

it('loads approved artifacts on demand without parent reads and pages independently', async () => {
  const test = fixture();
  test.state.second = true;
  render(<ArtifactButton client={test.client} id="agt_1" />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: 'Artifacts' }));
  await screen.findByRole('button', { name: 'Download Review.txt' });
  expect(screen.queryByRole('button', { name: 'Publish artifact' })).not.toBeInTheDocument();
  expect(
    test.paths.every(
      (path) =>
        path.includes('/resource-access/agent/agt_1') || path.includes('/agents/agt_1/artifacts'),
    ),
  ).toBe(true);
  await user.click(screen.getByRole('button', { name: 'Next artifacts' }));
  await screen.findByText('No published artifacts on this page.');
  expect(screen.queryByText(metadata.name)).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Previous artifacts' }));
  await screen.findByRole('button', { name: 'Download Review.txt' });
  test.state.revoked = true;
  await user.click(screen.getByRole('button', { name: 'Refresh artifacts' }));
  await waitFor(() => expect(screen.queryByText(metadata.name)).not.toBeInTheDocument());
});
it('validates bytes and current authority before creating a download', async () => {
  const test = fixture();
  const file = downloads();
  render(<AgentArtifacts client={test.client} id="agt_1" />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: 'Download Review.txt' }));
  await waitFor(() => expect(file.create).toHaveBeenCalledOnce());
  expect(file.click).toHaveBeenCalledOnce();
  test.state.corrupt = true;
  await user.click(screen.getByRole('button', { name: 'Download Review.txt' }));
  await screen.findByText('Artifact content did not match its published metadata.');
  expect(file.create).toHaveBeenCalledOnce();
});
it('refuses a policy change after artifact bytes were buffered', async () => {
  const test = fixture();
  await expect(
    authorizedArtifacts(test.client, 'agt_1', new AbortController().signal, async () => {
      test.state.policy++;
      return bytes;
    }),
  ).rejects.toThrow('Artifact access changed');
});
it('aborts and discards a late download after the artifact view is removed', async () => {
  const test = fixture();
  const file = downloads();
  const original = test.fetch.getMockImplementation()!;
  let finish: ((response: Response) => void) | undefined;
  test.fetch.mockImplementation(async (input) =>
    String(input).endsWith('/download')
      ? new Promise((resolve) => {
          finish = resolve;
        })
      : original(input),
  );
  const view = render(<AgentArtifacts client={test.client} id="agt_1" />);
  await userEvent.setup().click(await screen.findByRole('button', { name: 'Download Review.txt' }));
  await waitFor(() => expect(finish).toBeDefined());
  view.unmount();
  await act(async () =>
    finish?.(new Response(bytes, { headers: { 'content-type': 'application/octet-stream' } })),
  );
  expect(file.create).not.toHaveBeenCalled();
});
