import { act, render, screen } from '@testing-library/react';
import { ApiError, createClient } from '@chronoai/cma-sdk';
import { expect, it, vi } from 'vitest';
import { createRestWebClient } from '../api/rest';
import { WIRE_SANDBOX_DETAIL, wireRevision } from '../test/rest';
import type { CmaWebClient } from '../api/client';
import type { SandboxDetail } from '../api/types';
import { SandboxOverview } from './SandboxOverview';
import { useSandboxContents } from './useSandboxContents';

/** The Overview tab as the Sandbox page feeds it: from the shared contents read. */
function Overview({
  client,
  sandbox,
}: {
  readonly client: CmaWebClient;
  readonly sandbox: SandboxDetail;
}) {
  const { settings, settingsLoaded, contents, retry } = useSandboxContents({
    client,
    sandbox,
    readEnabled: true,
  });
  return (
    <SandboxOverview
      sandbox={sandbox}
      contents={contents}
      settings={settings}
      settingsLoaded={settingsLoaded}
      onRetry={retry}
      busy={false}
    />
  );
}

it.each([
  ['chronoai_managed', 'Chrono LLM (public)'],
  ['own_credentials', 'Your ChatGPT subscription'],
] as const)(
  'shows pinned %s providers without reading an internal profile',
  async (access, label) => {
    const fetch = vi.fn<typeof globalThis.fetch>(
      async (url) =>
        new Response(
          JSON.stringify(
            String(url).includes('/sandbox-profiles/')
              ? {
                  status: 404,
                  code: 'profile_not_found',
                  detail: 'No profile matches this request.',
                }
              : {
                  ...WIRE_SANDBOX_DETAIL,
                  agent_provider: 'codex',
                  model_access: access,
                  nyxid_services: [],
                  workspaces: [],
                },
          ),
          {
            status: String(url).includes('/sandbox-profiles/') ? 404 : 200,
            headers: { 'content-type': 'application/json' },
          },
        ),
    );
    const sdk = createClient({ fetch });
    await expect(sdk.sandboxProfiles.readRevision('profile-1', 1)).rejects.toBeInstanceOf(ApiError);
    const client = createRestWebClient(sdk);
    const sandbox = await client.readSandbox('sandbox-1');
    fetch.mockClear();
    render(<Overview client={client} sandbox={sandbox} />);
    await act(async () => {});
    expect(screen.getByText('Model access').parentElement).toHaveTextContent(label);
    // The agent provider is never named in CMA copy.
    expect(document.body.textContent).not.toMatch(/codex/i);
    expect(screen.queryByText('Unavailable')).not.toBeInTheDocument();
    expect(screen.queryByText(/Agent access settings could not be loaded/)).not.toBeInTheDocument();
    expect(fetch).not.toHaveBeenCalled();
  },
);

it('retains the pinned revision fallback for older Sandbox detail responses', async () => {
  const fetch = vi.fn<typeof globalThis.fetch>(
    async (url) =>
      new Response(
        JSON.stringify(
          String(url).includes('/sandbox-profiles/')
            ? { ...wireRevision('sandbox'), profile_id: 'profile-1' }
            : { ...WIRE_SANDBOX_DETAIL, nyxid_services: [], workspaces: [] },
        ),
        { status: 200, headers: { 'content-type': 'application/json' } },
      ),
  );
  const client = createRestWebClient(createClient({ fetch }));
  const sandbox = await client.readSandbox('sandbox-1');
  fetch.mockClear();
  render(<Overview client={client} sandbox={sandbox} />);
  expect(await screen.findByText('Your ChatGPT subscription')).toBeInTheDocument();
  expect(fetch).toHaveBeenCalledTimes(1);
  expect(fetch.mock.calls[0]?.[0]).toBe('/api/v1/sandbox-profiles/profile-1/revisions/1');
});
