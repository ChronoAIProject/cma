import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient, type AgentEnvironment } from '@chronoai/cma-sdk';
import { CmaClientProvider } from '../api/context';
import { agentClient } from '../test/agent';
import { ArtifactPublicationScope } from '../resources/artifactRecovery';
import { AgentSidePanel, type AgentPanel } from './AgentSidePanel';
import { SessionSettings } from './SessionSettings';
import { AGENT_DETAIL } from '../test/agent';
afterEach(() => vi.useRealTimers());

const feature = (state: AgentEnvironment['files']['state']) => ({
  state,
  reason: 'runtime_status',
  scope: 'sandbox',
  writable: state === 'available',
});
const environment = (state: AgentEnvironment['files']['state']): AgentEnvironment => ({
  agent_id: 'agt_1',
  runtime_incarnation: 'boot-one',
  policy_revision: 1,
  revalidate_at: null,
  files: feature(state),
  browser: feature('unsupported'),
  control: feature('unsupported'),
  replay: feature('unsupported'),
  operation_output: feature('available'),
});
const files = () => ({
  list: vi.fn(async () => []),
  read: vi.fn(),
  download: vi.fn(),
  write: vi.fn(),
  remove: vi.fn(),
});
const onClose = vi.fn();
const onAccessChanged = vi.fn();
function panel(
  client: ReturnType<typeof agentClient>,
  tab: AgentPanel = 'files',
  onSelect = vi.fn(),
  detail = AGENT_DETAIL,
  viewerId = 'owner-a',
) {
  return (
    <CmaClientProvider client={client}>
      <AgentSidePanel
        agentId="agt_1"
        shared={false}
        detail={detail}
        viewerId={viewerId}
        panel={tab}
        onSelect={onSelect}
        onClose={onClose}
        onAccessChanged={onAccessChanged}
        publicationScope={new ArtifactPublicationScope()}
      />
    </CmaClientProvider>
  );
}

it('waits for actual file availability and starts one read when the runtime becomes ready', async () => {
  let resolve!: (response: Response) => void;
  const boundary = vi.fn<typeof fetch>(
    () =>
      new Promise<Response>((done) => {
        resolve = done;
      }),
  );
  const sdk = createClient({ fetch: boundary });
  const fileClient = { ...files(), environment: sdk.agentFiles.environment.bind(sdk.agentFiles) };
  const client = agentClient({ files: fileClient });
  const view = render(panel(client));
  await waitFor(() => expect(boundary).toHaveBeenCalledTimes(1));
  expect(fileClient.list).not.toHaveBeenCalled();
  await act(async () => resolve(Response.json(environment('temporarily_unavailable'))));
  expect(await screen.findByText('Files is temporarily unavailable. Try again.')).toBeVisible();
  expect(fileClient.list).not.toHaveBeenCalled();
  fireEvent.focus(window);
  await waitFor(() => expect(boundary).toHaveBeenCalledTimes(2));
  await act(async () => resolve(Response.json(environment('available'))));
  await waitFor(() => expect(fileClient.list).toHaveBeenCalledTimes(1));
  view.rerender(panel(client, 'activity'));
  view.rerender(panel(client, 'files'));
  await act(async () => {});
  expect(fileClient.list).toHaveBeenCalledTimes(1);
});

it.each(['missing', '404', '403'] as const)(
  'uses only explicit legacy file fallback: %s',
  async (mode) => {
    const sdk = createClient({
      fetch: async () =>
        Response.json(
          { code: 'not_found', detail: 'Not available' },
          { status: Number(mode) || 404 },
        ),
    });
    const fileClient = {
      ...files(),
      environment: mode === 'missing' ? undefined : sdk.agentFiles.environment.bind(sdk.agentFiles),
    };
    render(panel(agentClient({ files: fileClient })));
    if (mode === '403') {
      await act(async () => {});
      expect(fileClient.list).not.toHaveBeenCalled();
    } else await waitFor(() => expect(fileClient.list).toHaveBeenCalledTimes(1));
  },
);

it('refreshes unavailable files when readiness changes and preserves an edited file across valid tabs', async () => {
  let available = false;
  const fileClient = {
    ...files(),
    environment: vi.fn(async () => environment(available ? 'available' : 'not_enabled')),
    list: vi.fn(async () => [
      { path: 'notes.txt', name: 'notes.txt', kind: 'file' as const, size_bytes: 5, hidden: false },
    ]),
    read: vi.fn(async () => ({
      path: 'notes.txt',
      size_bytes: 5,
      sha256: 'a'.repeat(64),
      content_base64: btoa('Notes'),
    })),
  };
  const client = agentClient({ files: fileClient });
  const starting = {
    ...AGENT_DETAIL,
    status: 'starting' as const,
    readyAt: undefined,
    sandbox: { ...AGENT_DETAIL.sandbox, status: 'installing' as const },
  };
  const view = render(panel(client, 'files', vi.fn(), starting));
  await screen.findByText('Files is not enabled for this environment.');
  expect(fileClient.list).not.toHaveBeenCalled();
  available = true;
  view.rerender(panel(client));
  fireEvent.click(await screen.findByRole('button', { name: 'notes.txt' }));
  const editor = await screen.findByRole('textbox', { name: 'Contents of notes.txt' });
  fireEvent.change(editor, { target: { value: 'Retain the unsaved draft' } });
  view.rerender(panel(client, 'activity'));
  view.rerender(panel(client, 'details'));
  view.rerender(panel(client));
  expect(screen.getByRole('textbox', { name: 'Contents of notes.txt' })).toHaveValue(
    'Retain the unsaved draft',
  );
  expect(fileClient.list).toHaveBeenCalledTimes(1);
});

it('selects the available browser without starting a hidden file read', async () => {
  const onSelect = vi.fn();
  const fileClient = {
    ...files(),
    environment: async () => ({ ...environment('available'), browser: feature('available') }),
  };
  render(panel(agentClient({ files: fileClient }), 'files', onSelect));
  await waitFor(() => expect(onSelect).toHaveBeenCalledWith('browser'));
  expect(fileClient.list).not.toHaveBeenCalled();
});

it('refreshes the desktop browser after Agent readiness even when files were already available', async () => {
  let ready = false;
  const sdk = createClient({
    fetch: async (input) =>
      Response.json(
        String(input).endsWith('/environment')
          ? {
              ...environment('available'),
              browser: feature(ready ? 'available' : 'not_enabled'),
            }
          : {
              agent_id: 'agt_1',
              runtime_incarnation: 'boot-one',
              context_id: 'context',
              state: 'live',
              location: 'Local page',
              observers: 1,
              captures: 1,
              bytes: 3,
              min_interval_ms: 1000,
              fresh_for_ms: 3000,
              max_frame_bytes: 262144,
              max_observers: 16,
              frame: {
                sequence: 1,
                captured_at: new Date().toISOString(),
                media_type: 'image/jpeg',
                width: 1280,
                height: 720,
                data: '/9j/',
              },
            },
      ),
  });
  const client = agentClient({
    files: { ...files(), environment: sdk.agentFiles.environment.bind(sdk.agentFiles) },
    browser: sdk.agentBrowser,
  });
  const starting = { ...AGENT_DETAIL, status: 'starting' as const, readyAt: undefined };
  const view = render(panel(client, 'browser', vi.fn(), starting));
  await screen.findByText('Browser is not enabled for this environment.');
  ready = true;
  view.rerender(panel(client, 'browser'));
  expect(await screen.findByAltText('Agent browser page')).toBeVisible();
  expect(
    screen.queryByText('Browser is not enabled for this environment.'),
  ).not.toBeInTheDocument();
});

it.each(['failure', 'expiry'] as const)(
  'keeps an unsaved file draft through authorization %s without file IO',
  async (mode) => {
    vi.useFakeTimers({ toFake: ['Date'] });
    let available = true;
    let resolve: (() => void) | undefined;
    const fileClient = {
      ...files(),
      environment: async () => {
        if (!available) {
          if (mode === 'failure') throw new TypeError('Temporary connection failure');
          await new Promise<void>((done) => {
            resolve = done;
          });
        }
        return environment('available');
      },
      list: vi.fn(async () => [
        {
          path: 'notes.txt',
          name: 'notes.txt',
          kind: 'file' as const,
          size_bytes: 5,
          hidden: false,
        },
      ]),
      read: vi.fn(async () => ({
        path: 'notes.txt',
        size_bytes: 5,
        sha256: 'a'.repeat(64),
        content_base64: btoa('Notes'),
      })),
    };
    render(panel(agentClient({ files: fileClient })));
    fireEvent.click(await screen.findByRole('button', { name: 'notes.txt' }));
    fireEvent.change(await screen.findByRole('textbox', { name: 'Contents of notes.txt' }), {
      target: { value: 'Retain across outage' },
    });
    available = false;
    if (mode === 'expiry') vi.setSystemTime(Date.now() + 55_001);
    fireEvent.focus(window);
    await waitFor(() =>
      expect(
        screen.queryByRole('textbox', { name: 'Contents of notes.txt' }),
      ).not.toBeInTheDocument(),
    );
    expect(screen.queryByRole('button', { name: 'Save' })).not.toBeInTheDocument();
    expect(fileClient.list).toHaveBeenCalledTimes(1);
    expect(fileClient.read).toHaveBeenCalledTimes(1);
    expect(fileClient.write).not.toHaveBeenCalled();
    available = true;
    if (resolve) await act(async () => resolve?.());
    else fireEvent.focus(window);
    expect(await screen.findByRole('textbox', { name: 'Contents of notes.txt' })).toHaveValue(
      'Retain across outage',
    );
    expect(fileClient.read).toHaveBeenCalledTimes(1);
    expect(fileClient.write).not.toHaveBeenCalled();
  },
);

it('cancels a delayed environment read before another account can display its files', async () => {
  const reads: { signal?: AbortSignal; resolve: (value: AgentEnvironment) => void }[] = [];
  const fileClient = {
    ...files(),
    environment: vi.fn(
      (_agent: string, options?: { signal?: AbortSignal }) =>
        new Promise<AgentEnvironment>((resolve) =>
          reads.push({ signal: options?.signal, resolve }),
        ),
    ),
  };
  const client = agentClient({ files: fileClient });
  const view = render(panel(client));
  await waitFor(() => expect(reads).toHaveLength(1));
  view.rerender(panel(client, 'files', vi.fn(), AGENT_DETAIL, 'owner-b'));
  await waitFor(() => expect(reads).toHaveLength(2));
  expect(reads[0]!.signal?.aborted).toBe(true);
  await act(async () => reads[0]!.resolve(environment('available')));
  expect(fileClient.list).not.toHaveBeenCalled();
  await act(async () => reads[1]!.resolve(environment('available')));
  await waitFor(() => expect(fileClient.list).toHaveBeenCalledTimes(1));
});

it('keeps stored settings visible and defers only models until ready, including recovery', async () => {
  const readAgentModels = vi.fn(async () => [
    { id: 'saved-model', label: 'Saved model', available: true },
  ]);
  const client = agentClient({
    readAgentModels,
    readAgentSettings: async () => ({
      model: 'saved-model',
      reasoning_effort: 'high',
      permission_mode: 'autonomous',
    }),
    updateAgentSettings: vi.fn(),
  });
  const draw = (runtimeReady: boolean) => (
    <CmaClientProvider client={client}>
      <SessionSettings agentId="agt_1" active busy runtimeReady={runtimeReady} />
    </CmaClientProvider>
  );
  const view = render(draw(false));
  expect(await screen.findByRole('button', { name: /^Agent settings:/ })).toBeVisible();
  expect(readAgentModels).not.toHaveBeenCalled();
  view.rerender(draw(true));
  await waitFor(() => expect(readAgentModels).toHaveBeenCalledTimes(1));
  view.rerender(draw(false));
  expect(screen.getByRole('button', { name: /^Agent settings:/ })).toBeVisible();
  view.rerender(draw(true));
  await waitFor(() => expect(readAgentModels).toHaveBeenCalledTimes(2));
});
