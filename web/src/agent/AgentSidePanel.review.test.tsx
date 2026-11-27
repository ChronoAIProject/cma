import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { useState } from 'react';
import { CmaClientProvider } from '../api/context';
import { agentClient, AGENT_DETAIL } from '../test/agent';
import { ArtifactPublicationScope } from '../resources/artifactRecovery';
import { AgentSidePanel, type AgentPanel } from './AgentSidePanel';
import { conversationFromTranscript } from './conversation';
import type { ToolCall } from './model';
import { environmentFixture } from './environmentReview.fixture';
vi.mock('./DesktopPanel', () => ({
  DesktopPanel: ({ application }: { application: string }) => (
    <div>Native desktop · {application}</div>
  ),
}));
vi.mock('./environment/BrowserPanel', () => ({
  BrowserPanel: () => <div>Agent browser view</div>,
  EnableBrowser: () => null,
}));
afterEach(() => {
  vi.restoreAllMocks();
  Reflect.deleteProperty(HTMLElement.prototype, 'requestFullscreen');
  Reflect.deleteProperty(document, 'fullscreenElement');
  Reflect.deleteProperty(document, 'exitFullscreen');
});
const base = conversationFromTranscript({ agentId: 'agt_1', throughSeq: 0, turns: [] });
const scope = new ArtifactPublicationScope();
const client = agentClient({
  files: {
    environment: async () => environmentFixture,
    list: async () => [],
    read: vi.fn(),
    download: vi.fn(),
    write: vi.fn(),
    remove: vi.fn(),
  },
  browser: { read: vi.fn(), release: vi.fn(), configure: vi.fn() },
});
function draw(
  tools: readonly ToolCall[] = [],
  loading = false,
  panel: AgentPanel = 'browser',
  onSelect: (panel: AgentPanel) => void = vi.fn(),
) {
  return (
    <CmaClientProvider client={client}>
      <AgentSidePanel
        agentId="agt_1"
        viewerId="owner"
        shared={false}
        detail={{ ...AGENT_DETAIL, desktop: true }}
        panel={panel}
        onSelect={onSelect}
        onClose={vi.fn()}
        onAccessChanged={vi.fn()}
        publicationScope={scope}
        conversation={{ ...base, toolCalls: tools }}
        historyLoading={loading}
      />
    </CmaClientProvider>
  );
}
it('follows actual browser and desktop commands without a manual source toggle', async () => {
  const view = render(draw());
  await screen.findByText('Native desktop · Desktop');
  view.rerender(
    draw([
      {
        id: 'browser',
        title: 'cma-browser navigate https://example.com',
        kind: 'execute',
        status: 'completed',
      },
    ]),
  );
  expect(await screen.findByText('Agent browser view')).toBeVisible();
  expect(screen.queryByText(/Native desktop/)).not.toBeInTheDocument();
  view.rerender(
    draw([
      { id: 'desktop', title: 'xdotool windowactivate 44', kind: 'execute', status: 'completed' },
    ]),
  );
  expect(await screen.findByText('Native desktop · Desktop')).toBeVisible();
  expect(screen.queryByRole('button', { name: /Shared desktop|Agent browser/ })).toBeNull();
});
it('preserves a requested Operations tab during loading and redirects only after confirmed empty history', async () => {
  const select = vi.fn();
  const view = render(draw([], true, 'activity', select));
  await act(async () => {});
  expect(select).not.toHaveBeenCalled();
  expect(screen.getByRole('tab', { name: 'Operations' })).toHaveAttribute('aria-selected', 'true');
  expect(screen.getByText('Loading recorded operations…')).toBeVisible();
  view.rerender(
    draw(
      [{ id: 'command', kind: 'execute', title: 'pwd', status: 'completed' }],
      false,
      'activity',
      select,
    ),
  );
  expect(screen.getByRole('tab', { name: 'Operations' })).toHaveAttribute('aria-selected', 'true');
  expect(select).not.toHaveBeenCalled();
  view.rerender(draw([], false, 'activity', select));
  await waitFor(() => expect(select).toHaveBeenCalledWith('browser', 'activity'));
});
it('clears fullscreen errors on tab changes, reconnect, and successful fullscreen events', async () => {
  Object.defineProperty(HTMLElement.prototype, 'requestFullscreen', {
    configurable: true,
    value: vi.fn().mockRejectedValue(new Error('refused')),
  });
  function View() {
    const [panel, select] = useState<AgentPanel>('browser');
    return draw([], false, panel, select);
  }
  render(<View />);
  const refusal =
    'The browser declined full screen. Allow full screen in your browser and try again.';
  const fail = async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Full screen' }));
    await screen.findByText(refusal);
  };
  await fail();
  fireEvent.click(screen.getByRole('tab', { name: 'Files' }));
  expect(screen.queryByText(refusal)).toBeNull();
  fireEvent.click(screen.getByRole('tab', { name: 'Screen' }));
  await fail();
  fireEvent.click(screen.getByRole('button', { name: 'Reconnect' }));
  expect(screen.queryByText(refusal)).toBeNull();
  await fail();
  fireEvent(document, new Event('fullscreenchange'));
  expect(screen.queryByText(refusal)).toBeNull();
});

it('uses native fullscreen when granted and retains the panel through Escape', async () => {
  const request = vi.fn(async function (this: HTMLElement) {
    Object.defineProperty(document, 'fullscreenElement', { configurable: true, value: this });
    document.dispatchEvent(new Event('fullscreenchange'));
  });
  const exit = vi.fn(async () => {
    Object.defineProperty(document, 'fullscreenElement', { configurable: true, value: null });
    document.dispatchEvent(new Event('fullscreenchange'));
  });
  Object.defineProperty(HTMLElement.prototype, 'requestFullscreen', {
    configurable: true,
    value: request,
  });
  Object.defineProperty(document, 'exitFullscreen', { configurable: true, value: exit });

  render(draw());
  await screen.findByText('Native desktop · Desktop');
  const panel = screen.getByRole('complementary', { name: 'Agent computer' });
  fireEvent.click(screen.getByRole('button', { name: 'Full screen' }));
  expect(request).toHaveBeenCalledOnce();
  expect(document.fullscreenElement).toBe(panel);
  expect(screen.getByRole('button', { name: 'Exit full screen' })).toBeVisible();
  fireEvent.keyDown(panel, { key: 'Escape' });
  expect(exit).toHaveBeenCalledOnce();
  expect(screen.getByRole('button', { name: 'Full screen' })).toBeVisible();
  expect(screen.getByText('Native desktop · Desktop')).toBeVisible();
});
