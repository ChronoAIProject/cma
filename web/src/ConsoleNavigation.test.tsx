import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from './api/context';
import type { User } from './auth/types';
import { Console } from './Console';
import type { AppRoute } from './routing/routes';
import { SIDEBAR_MIN } from './shell/columns';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, AGENT_USAGE, agentClient } from './test/agent';

const OWNER: User = {
  id: 'owner',
  handle: 'Owner',
  visitor: false,
  capabilities: ['use_own_sandboxes'],
};

afterEach(() => {
  vi.restoreAllMocks();
});

it.each([
  { width: 390, initialAgent: 'agt_1' },
  { width: 390, initialAgent: 'agt_2' },
  { width: 900, initialAgent: 'agt_1' },
  { width: 900, initialAgent: 'agt_2' },
  { width: 1440, initialAgent: 'agt_1' },
  { width: 1440, initialAgent: 'agt_2' },
])(
  'reveals the selected Agent at $width px from $initialAgent',
  async ({ width, initialAgent }) => {
    const matchMedia = window.matchMedia.bind(window);
    vi.spyOn(window, 'matchMedia').mockImplementation((query) => ({
      ...matchMedia(query),
      matches: query === '(min-width: 1024px)' && width >= 1024,
    }));
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({
      width,
      height: 844,
    } as DOMRect);
    const actor = userEvent.setup();
    const onNavigate = vi.fn();
    const runAgent = vi.fn();
    const client = agentClient({
      readAgent: async (id) => ({
        ...AGENT_DETAIL,
        id,
        title: id === 'agt_1' ? 'Agent one' : 'Agent two',
        worktree: { ...AGENT_DETAIL.worktree, agentId: id },
      }),
      readAgentTranscript: async (id) => ({ ...AGENT_TRANSCRIPT, agentId: id }),
      readAgentUsage: async (id) => ({ ...AGENT_USAGE, agentId: id }),
      runAgent,
    });
    function NavigableConsole() {
      const [route, setRoute] = useState<AppRoute>({ kind: 'agent', agentId: initialAgent });
      return (
        <CmaClientProvider client={client}>
          <Console
            user={OWNER}
            route={route}
            onNavigate={(next) => {
              onNavigate(next);
              setRoute(next);
            }}
            onSignOut={vi.fn()}
          />
        </CmaClientProvider>
      );
    }
    render(<NavigableConsole />);
    expect(
      await screen.findByRole('heading', {
        name: initialAgent === 'agt_1' ? 'Agent one' : 'Agent two',
      }),
    ).toBeInTheDocument();
    expect(screen.queryByRole('complementary', { name: 'Agent computer' })).not.toBeInTheDocument();
    if (initialAgent === 'agt_1') {
      await actor.type(screen.getByRole('textbox', { name: 'Message' }), 'Keep this draft');
    }
    if (width < 640) {
      await actor.click(screen.getByRole('button', { name: 'Show navigation' }));
    }
    const frame = document.querySelector('.frame') as HTMLElement;
    if (width === 390) {
      expect(frame).toHaveAttribute('data-sidebar-overlay');
      expect(document.querySelector('.frame__center')).toHaveAttribute('inert');
    }
    await actor.click(await screen.findByRole('link', { name: /Agent one/ }));

    expect(onNavigate).toHaveBeenCalledExactlyOnceWith({ kind: 'agent', agentId: 'agt_1' });
    expect(await screen.findByRole('heading', { name: 'Agent one' })).toBeInTheDocument();
    expect(frame).not.toHaveAttribute('data-sidebar-overlay');
    await waitFor(() =>
      expect(
        frame.style.gridTemplateColumns.startsWith(
          // A phone has no rail: the top bar is the way back to navigation.
          `${width < 640 ? 0 : SIDEBAR_MIN}px`,
        ),
      ).toBe(true),
    );
    expect(
      screen.getByRole('button', { name: width < 640 ? 'Show navigation' : 'Hide navigation' }),
    ).toBeInTheDocument();
    expect(screen.getByText('The repository is ready.')).toBeInTheDocument();
    if (initialAgent === 'agt_1') {
      expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue('Keep this draft');
    }
    expect(runAgent).not.toHaveBeenCalled();
  },
);
it('keeps the conversation mounted through explicit panel choices and resets on navigation', async () => {
  const actor = userEvent.setup();
  const runAgent = vi.fn();
  const client = agentClient({
    readAgent: async (id) => ({ ...AGENT_DETAIL, id }),
    readAgentTranscript: async (id) => ({ ...AGENT_TRANSCRIPT, agentId: id }),
    readAgentUsage: async (id) => ({ ...AGENT_USAGE, agentId: id }),
    runAgent,
  });
  const draw = (agentId: string) => (
    <CmaClientProvider client={client}>
      <Console
        user={OWNER}
        route={{ kind: 'agent', agentId }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>
  );
  const view = render(draw('agt_1'));
  const history = await screen.findByText('The repository is ready.');
  const composer = screen.getByRole('textbox', { name: 'Message' });
  await actor.type(composer, 'Keep this unsent draft');
  const scroll = document.querySelector('.agent-surface__conversation')!;
  Object.defineProperties(scroll, {
    scrollHeight: { value: 2000 },
    clientHeight: { value: 400 },
  });
  scroll.scrollTop = 1600;
  fireEvent.scroll(scroll);
  scroll.scrollTop = 100;
  fireEvent.scroll(scroll);
  const expectConversation = () => {
    expect(screen.getByRole('textbox', { name: 'Message' })).toBe(composer);
    expect(composer).toHaveValue('Keep this unsent draft');
    expect(screen.getByText('The repository is ready.')).toBe(history);
    expect(document.querySelector('.agent-surface__conversation')).toBe(scroll);
    expect(scroll.scrollTop).toBe(100);
  };
  await actor.click(screen.getByRole('button', { name: 'Show Agent computer' }));
  expect(screen.getByRole('complementary', { name: 'Agent computer' })).toBeVisible();
  expectConversation();
  view.rerender(draw('agt_1'));
  expect(screen.getByRole('complementary', { name: 'Agent computer' })).toBeVisible();
  await actor.click(screen.getByRole('button', { name: 'Close computer' }));
  await actor.click(screen.getByRole('button', { name: 'Show Agent details' }));
  expectConversation();
  await actor.click(screen.getByRole('button', { name: 'Hide Agent details' }));
  fireEvent(window, new Event('resize'));
  fireEvent(window, new Event('focus'));
  expect(screen.queryByRole('complementary', { name: 'Agent computer' })).not.toBeInTheDocument();
  expectConversation();
  await actor.click(screen.getByRole('button', { name: 'Show Agent computer' }));
  view.rerender(draw('agt_2'));
  await screen.findByText('The repository is ready.');
  expect(screen.queryByRole('complementary', { name: 'Agent computer' })).not.toBeInTheDocument();
  view.rerender(draw('agt_1'));
  await screen.findByText('The repository is ready.');
  expect(screen.queryByRole('complementary', { name: 'Agent computer' })).not.toBeInTheDocument();
  expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue('Keep this unsent draft');
  expect(runAgent).not.toHaveBeenCalled();
});

import './test/connectedAccounts';
