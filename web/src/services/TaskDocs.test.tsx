import { fireEvent, render, screen, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { TaskDocs } from './TaskDocs';
import { DocsPage } from './DocsPage';
import type { HelpTopic } from '../routing/docsRoutes';

it('puts the four everyday tasks before the separate integration reference', () => {
  render(<TaskDocs onNavigate={vi.fn()} />);
  expect(screen.getAllByRole('link').map((link) => link.getAttribute('href'))).toEqual([
    '/docs/start-a-task',
    '/docs/environments-and-capacity',
    '/docs/recover-and-continue',
    '/docs/find-your-outputs',
    '/docs/event-gateway',
  ]);
  expect(screen.getByRole('heading', { name: 'Build an integration' })).toBeVisible();
  expect(
    screen.getAllByRole('heading', { level: 2 }).map((heading) => heading.textContent),
  ).toEqual([
    'Start a task',
    'Environments and capacity',
    'Recover and continue',
    'Find your outputs',
    'Build an integration',
  ]);
});

it.each<{ view: HelpTopic; title: string }>([
  { view: 'start-a-task', title: 'Start a task' },
  { view: 'environments-and-capacity', title: 'Environments and capacity' },
  { view: 'recover-and-continue', title: 'Recover and continue' },
  { view: 'find-your-outputs', title: 'Find your outputs' },
])(
  'opens and focuses $title with state, action, and completion guidance',
  async ({ view, title }) => {
    const navigate = vi.fn();
    render(<DocsPage view={view} onNavigate={navigate} />);
    expect(await screen.findByRole('heading', { level: 1, name: title })).toHaveFocus();
    for (const scenario of screen.getByRole('article').querySelectorAll('section')) {
      expect(
        within(scenario)
          .getAllByRole('term')
          .map((term) => term.textContent),
      ).toEqual(['What you see', 'What you can do next', 'How to know it completed']);
      for (const definition of within(scenario).getAllByRole('definition')) {
        expect(definition).not.toBeEmptyDOMElement();
      }
    }
    fireEvent.click(screen.getByRole('link', { name: 'Docs' }));
    expect(navigate).toHaveBeenCalledExactlyOnceWith({ kind: 'docs' });
  },
);

it('opens the Event Gateway documentation from the Docs index using application navigation', () => {
  const navigate = vi.fn();
  render(<TaskDocs onNavigate={navigate} />);
  const link = screen.getByRole('link', { name: /Event Gateway Protocol/ });
  expect(link).toHaveAttribute('href', '/docs/event-gateway');
  fireEvent.click(link);
  expect(navigate).toHaveBeenCalledExactlyOnceWith({
    kind: 'docs',
    view: 'event_gateway_protocol',
  });
});

it('names the task and Artifact recovery actions shown by the application', () => {
  render(<DocsPage view="recover-and-continue" onNavigate={vi.fn()} />);
  const guidance = screen.getByText(/For a task, return to the original task/);
  expect(guidance).toHaveTextContent('Recover original task');
  expect(guidance).toHaveTextContent('Retry creation in the open creation dialog');
  expect(guidance).toHaveTextContent('Resume creation in Artifacts to reopen the saved request');
});

it('uses the Live Sandbox status in the paused task deletion guide', () => {
  render(<DocsPage view="environments-and-capacity" onNavigate={vi.fn()} />);
  const guide = within(screen.getByRole('article'));
  expect(guide.getByRole('heading', { name: 'Delete a task in a paused Sandbox' })).toBeVisible();
  expect(
    guide.getByText('A stopped or failed Agent can be deleted after its Sandbox returns to Live.'),
  ).toBeVisible();
  expect(
    guide.getByText(/Wait until its status is Live, then return to the task and choose Delete/),
  ).toBeVisible();
  expect(
    guide.getByText(/An environment marked Deleting still occupies a Sandbox slot/),
  ).toBeVisible();
});
