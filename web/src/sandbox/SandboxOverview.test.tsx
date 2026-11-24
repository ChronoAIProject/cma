import { act, fireEvent, render, screen } from '@testing-library/react';
import { ApiError, createClient } from '@cma/client';
import { expect, it, vi } from 'vitest';
import { createRestWebClient } from '../api/rest';
import { WIRE_SANDBOX_DETAIL, wireRevision } from '../test/rest';
import type { CmaWebClient } from '../api/client';
import type { SandboxDetail } from '../api/types';
import { SandboxOverview } from './SandboxOverview';
import { useSandboxContents } from './useSandboxContents';
import { sandboxFixture } from '../test/hierarchy';

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

it.each([undefined, false])('offers no enable action when Always on is %s', (alwaysOn) => {
  const change = vi.fn();
  render(
    <SandboxOverview
      sandbox={{ ...sandboxFixture(), alwaysOn }}
      settingsLoaded
      busy={false}
      onRetry={vi.fn()}
      onAlwaysOnChange={change}
    />,
  );
  expect(
    screen.getByText(/Always on is off. Only an administrator can approve an exception/),
  ).toBeInTheDocument();
  expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /Always on/ })).not.toBeInTheDocument();
  expect(change).not.toHaveBeenCalled();
});

it('only relinquishes an existing exception and shows the resulting state', async () => {
  const change = vi.fn(async () => {});
  const props = { settingsLoaded: true, busy: false, onRetry: vi.fn(), onAlwaysOnChange: change };
  const view = render(
    <SandboxOverview {...props} sandbox={{ ...sandboxFixture(), alwaysOn: true }} />,
  );
  expect(
    screen.getByText(/An administrator approved Always on, so automatic pause skips this Sandbox/),
  ).toBeInTheDocument();
  await act(async () =>
    fireEvent.click(screen.getByRole('button', { name: 'Turn off Always on' })),
  );
  expect(change).toHaveBeenCalledExactlyOnceWith(false);
  view.rerender(<SandboxOverview {...props} sandbox={{ ...sandboxFixture(), alwaysOn: false }} />);
  expect(screen.queryByRole('button', { name: /Always on/ })).not.toBeInTheDocument();
});

it.each([{ busy: true }, { stale: true }])(
  'blocks relinquishment while unavailable: %j',
  (state) => {
    render(
      <SandboxOverview
        sandbox={{ ...sandboxFixture(), alwaysOn: true }}
        settingsLoaded
        busy={false}
        {...state}
        onRetry={vi.fn()}
        onAlwaysOnChange={vi.fn()}
      />,
    );
    expect(screen.getByRole('button', { name: 'Turn off Always on' })).toBeDisabled();
  },
);

it('keeps the exception visible and reports a failed relinquishment', async () => {
  render(
    <SandboxOverview
      sandbox={{ ...sandboxFixture(), alwaysOn: true }}
      settingsLoaded
      busy={false}
      onRetry={vi.fn()}
      onAlwaysOnChange={async () => {
        throw new Error('Revision changed');
      }}
    />,
  );
  await act(async () =>
    fireEvent.click(screen.getByRole('button', { name: 'Turn off Always on' })),
  );
  expect(screen.getByRole('alert')).toHaveTextContent('Revision changed');
  expect(screen.getByRole('button', { name: 'Turn off Always on' })).toBeEnabled();
});

it.each([
  ['retired', 'This environment is no longer running. Live resource usage is no longer available.'],
  ['retiring', 'Live resource usage is unavailable while this environment is retiring.'],
] as const)(
  'does not invite a new Agent to obtain live resources in a %s environment',
  (status, message) => {
    render(
      <SandboxOverview
        sandbox={sandboxFixture(status)}
        settingsLoaded
        busy={false}
        onRetry={vi.fn()}
      />,
    );
    expect(screen.getByText(message)).toBeVisible();
    expect(
      screen.queryByText('Resources are available when this Sandbox has an Agent.'),
    ).not.toBeInTheDocument();
  },
);
