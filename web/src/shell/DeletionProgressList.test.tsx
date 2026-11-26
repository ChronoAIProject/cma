import { fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { DeletionProgressList } from './DeletionProgressList';
import { delayedRetirementFixture } from '../test/hierarchy';

const sandboxes = [
  { id: 'sbx_synthetic_identifier', name: 'Readable environment name', status: 'active' as const },
  { id: 'sbx_other_identifier', name: 'Another environment', status: 'retiring' as const },
];
const clipboard = Object.getOwnPropertyDescriptor(navigator, 'clipboard');
afterEach(() => {
  if (clipboard) Object.defineProperty(navigator, 'clipboard', clipboard);
  else Reflect.deleteProperty(navigator, 'clipboard');
});

it('keeps accepted and retiring status truthful and each identifier action separate', async () => {
  const user = userEvent.setup();
  const write = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue();
  const onViewStatus = vi.fn();
  const view = render(<DeletionProgressList sandboxes={sandboxes} onViewStatus={onViewStatus} />);
  await user.click(screen.getByText('Cleaning up 2 sandboxes'));
  const rows = screen.getAllByRole('listitem');
  expect(within(rows[0]!).getByText('Deletion requested')).toBeVisible();
  expect(within(rows[1]!).getByText('Deleting')).toBeVisible();
  expect(view.container.querySelector('button button')).toBeNull();
  expect(view.container.querySelector('.deletion-progress__full')).toBeNull();
  await user.click(
    within(rows[1]!).getByRole('button', { name: 'Copy identifier for Another environment' }),
  );
  expect(write).toHaveBeenCalledExactlyOnceWith(sandboxes[1]!.id);
  expect(within(rows[1]!).getByRole('status')).toHaveTextContent('Identifier copied.');
  expect(within(rows[0]!).queryByRole('status')).not.toBeInTheDocument();
  expect(onViewStatus).not.toHaveBeenCalled();
  await user.click(
    within(rows[0]!).getByRole('button', { name: 'View status for Readable environment name' }),
  );
  expect(onViewStatus).toHaveBeenCalledExactlyOnceWith(sandboxes[0]!.id);
  write.mockRestore();
});

it.each(['denied', 'missing'] as const)(
  'offers selectable full identifiers when clipboard is %s',
  async (failure) => {
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value:
        failure === 'missing'
          ? undefined
          : {
              writeText: vi
                .fn()
                .mockRejectedValue(new DOMException('Synthetic denial', 'NotAllowedError')),
            },
    });
    const onViewStatus = vi.fn();
    const view = render(<DeletionProgressList sandboxes={sandboxes} onViewStatus={onViewStatus} />);
    fireEvent.click(screen.getByText('Cleaning up 2 sandboxes'));
    fireEvent.click(
      screen.getByRole('button', { name: 'Copy identifier for Readable environment name' }),
    );
    expect(await screen.findByRole('status')).toHaveTextContent(
      'Copy unavailable. Select the full identifier below.',
    );
    expect(view.container.querySelector('.deletion-progress__full')).toHaveTextContent(
      sandboxes[0]!.id,
    );
    expect(onViewStatus).not.toHaveBeenCalled();
  },
);

it('does not render an empty disclosure', () => {
  const { container } = render(<DeletionProgressList sandboxes={[]} onViewStatus={vi.fn()} />);
  expect(container).toBeEmptyDOMElement();
});

it('keeps UNKNOWN requests distinct in a mixed footer', async () => {
  const user = userEvent.setup();
  render(
    <DeletionProgressList
      sandboxes={[
        sandboxes[0]!,
        { id: 'sbx_unknown', name: 'Unconfirmed environment', unknown: true },
      ]}
      onViewStatus={vi.fn()}
    />,
  );
  await user.click(screen.getByText('2 deletions need checking'));
  expect(screen.getByText('Deletion not confirmed')).toBeVisible();
  expect(screen.getByText('Deletion requested')).toBeVisible();
  expect(screen.queryByText('Deleting')).not.toBeInTheDocument();
});

it('does not treat a delayed non-retirement operation as a deletion deadline', async () => {
  const delayed = delayedRetirementFixture();
  const user = userEvent.setup();
  render(
    <DeletionProgressList
      sandboxes={[
        delayed,
        { ...delayed, id: 'sbx_other', name: 'Another Sandbox', status: 'reviving' },
      ]}
      onViewStatus={vi.fn()}
    />,
  );
  await user.click(screen.getByText('2 deletions need checking'));
  const rows = screen.getAllByRole('listitem');
  expect(within(rows[0]!).getByText('Cleanup taking longer')).toBeVisible();
  expect(within(rows[1]!).getByText('Deletion requested')).toBeVisible();
  expect(within(rows[0]!).getByText(/expected deletion time has passed/)).toBeVisible();
  expect(within(rows[1]!).queryByText(/expected deletion time has passed/)).not.toBeInTheDocument();
});
