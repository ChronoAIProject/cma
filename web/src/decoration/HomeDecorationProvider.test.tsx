import { useContext } from 'react';
import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { TaskHomeDecorationContext } from '../create/TaskHomeDecoration';
import { HomeDecorationProvider } from './HomeDecorationProvider';

const backdrop = vi.hoisted(() => vi.fn());
vi.mock('./TopographicBackdrop', () => ({ TopographicBackdrop: backdrop }));
beforeEach(() => backdrop.mockReset().mockImplementation(() => <span data-testid="decoration" />));

function Slot({ eligible }: { readonly eligible: boolean }) {
  const decoration = useContext(TaskHomeDecorationContext);
  return (
    <>
      <input aria-label="Draft" />
      {eligible ? decoration : null}
    </>
  );
}

it('provision alone does not mount or initialize the decoration', () => {
  render(
    <HomeDecorationProvider>
      <Slot eligible={false} />
    </HomeDecorationProvider>,
  );
  expect(backdrop).not.toHaveBeenCalled();
});

it('renders only when the owner slot consumes the node, preserving the same draft across transitions', () => {
  const view = render(
    <HomeDecorationProvider>
      <Slot eligible />
    </HomeDecorationProvider>,
  );
  expect(screen.getByTestId('decoration')).toBeInTheDocument();
  expect(backdrop.mock.calls[0]![0]).toEqual({ placement: 'first-visit' });
  const input = screen.getByRole('textbox', { name: 'Draft' });
  fireEvent.change(input, { target: { value: 'Keep this draft' } });
  view.rerender(
    <HomeDecorationProvider>
      <Slot eligible={false} />
    </HomeDecorationProvider>,
  );
  expect(screen.queryByTestId('decoration')).not.toBeInTheDocument();
  expect(screen.getByRole('textbox', { name: 'Draft' })).toBe(input);
  expect(input).toHaveValue('Keep this draft');
  view.rerender(
    <HomeDecorationProvider>
      <Slot eligible />
    </HomeDecorationProvider>,
  );
  expect(screen.getByTestId('decoration')).toBeInTheDocument();
  expect(screen.getByRole('textbox', { name: 'Draft' })).toBe(input);
  expect(input).toHaveValue('Keep this draft');
});
