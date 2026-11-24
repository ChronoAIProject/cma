import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { RuntimeAttention } from '../shell/runtimeAttention';
import { navigationFixture } from '../test/hierarchy';
import { emptyTemplate } from './taskTemplateModel';
import { HomeRuntimeAttention } from './HomeRuntimeAttention';
import { TaskHomeLayout } from './TaskHomeLayout';

const failedSandboxes = [
  { sandboxId: 'sbx_one', name: 'First environment', agentIds: ['agt_one', 'agt_two'] },
  { sandboxId: 'sbx_two', name: 'Second environment', agentIds: [] },
];
const attention: RuntimeAttention = { freshness: 'fresh', failedSandboxes };

it('counts the shared Sandbox observations and only navigates on Review', () => {
  const onReview = vi.fn();
  render(<HomeRuntimeAttention attention={attention} onReview={onReview} />);
  expect(screen.getByRole('alert')).toHaveTextContent('2 sandboxes couldn’t update their runtime');
  expect(screen.queryByText(/agents.*safe|history.*safe|retry|delete/i)).not.toBeInTheDocument();
  expect(screen.getAllByRole('button')).toHaveLength(1);
  expect(onReview).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Review' }));
  expect(onReview).toHaveBeenCalledTimes(1);
});

it('marks retained observations as last-known and removes them when unknown or empty', () => {
  const view = render(<HomeRuntimeAttention attention={{ freshness: 'stale', failedSandboxes }} />);
  expect(screen.getByRole('status')).toHaveTextContent(
    'Last known: 2 sandboxes couldn’t update their runtime.',
  );
  expect(screen.getByText('Current runtime status is unconfirmed.')).toBeVisible();
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  for (const value of [
    undefined,
    { freshness: 'unknown', failedSandboxes },
    { freshness: 'fresh', failedSandboxes: [] },
    { freshness: 'stale', failedSandboxes: [] },
  ] as const) {
    view.rerender(<HomeRuntimeAttention attention={value} />);
    expect(view.container).toBeEmptyDOMElement();
  }
});

it('uses singular wording without inventing a navigation destination', () => {
  render(
    <HomeRuntimeAttention
      attention={{ freshness: 'fresh', failedSandboxes: failedSandboxes.slice(0, 1) }}
    />,
  );
  expect(screen.getByText('1 sandbox couldn’t update its runtime')).toBeVisible();
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
});

it('places attention before crew in ordinary personal Home and hides it in other hosts', () => {
  const onReviewRuntime = vi.fn();
  const props = {
    titleId: 'synthetic-home-title',
    contextual: false,
    submitted: false,
    onNavigate: vi.fn(),
    children: <p>Composer placeholder</p>,
    home: {
      navigation: { status: 'ready' as const, items: navigationFixture() },
      attention,
      onReviewRuntime,
    },
  };
  const view = render(<TaskHomeLayout {...props} />);
  const banner = screen.getByRole('alert');
  const crew = screen.getByRole('heading', { name: 'Your crew' });
  expect(banner.compareDocumentPosition(crew) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Review' }));
  expect(onReviewRuntime).toHaveBeenCalledTimes(1);
  for (const override of [
    { home: { ...props.home, scope: 'shared' } },
    { home: { ...props.home, scope: 'org:synthetic' } },
    { contextual: true },
    { submitted: true },
    { template: emptyTemplate('research') },
  ]) {
    view.rerender(<TaskHomeLayout {...props} {...override} />);
    expect(screen.queryByText('Needs you')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Review' })).not.toBeInTheDocument();
  }
});
