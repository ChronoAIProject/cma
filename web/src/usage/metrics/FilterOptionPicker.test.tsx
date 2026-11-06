import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ApiError, type UsageFilterOptionsQuery } from '@chronoai/cma-sdk';
import { expect, it, vi } from 'vitest';
import { FilterOptionPicker } from './FilterOptionPicker';
import { metricsClient } from './testFixtures';

const owner = {
  value: '91892afa-5fa5-4f4f-9100-2b8d986d2f71',
  label: 'Alex Chen · account a12bc34de567',
};
for (const count of [400, 20001]) {
  it(`searches and pages ${count} choices with a bounded DOM, keyboard selection and clear`, async () => {
    const actor = userEvent.setup();
    const api = metricsClient(true);
    const change = vi.fn();
    api.options.mockImplementation(async (query: UsageFilterOptionsQuery) => {
      const start = query.search ? count - 1 : Number(query.cursor ?? 0);
      return {
        items: Array.from(
          { length: Math.min(query.search ? 1 : 50, count - start) },
          (_, index) => ({
            value: `sbx_${String(start + index).padStart(32, '0')}`,
            label: `Research ${start + index} · Sandbox reference`,
          }),
        ),
        next: start + 50 < count ? String(start + 50) : null,
      };
    });
    const view = render(
      <FilterOptionPicker api={api} kind="sandbox" value="" onChange={change} active />,
    );
    await actor.click(screen.getByRole('button', { name: 'Sandbox: All Sandboxes' }));
    await screen.findByRole('menuitemradio', { name: 'Research 0 · Sandbox reference' });
    expect(screen.getAllByRole('menuitemradio')).toHaveLength(51);
    await actor.click(screen.getByRole('button', { name: 'Next choices' }));
    await screen.findByRole('menuitemradio', { name: 'Research 50 · Sandbox reference' });
    expect(screen.getAllByRole('menuitemradio')).toHaveLength(51);
    const search = screen.getByRole('textbox', { name: 'Search sandbox choices' });
    await actor.click(search);
    await actor.type(search, `Research ${count - 1}`);
    await screen.findByRole('menuitemradio', { name: `Research ${count - 1} · Sandbox reference` });
    expect(screen.getAllByRole('menuitemradio')).toHaveLength(2);
    expect(api.options).toHaveBeenLastCalledWith(
      expect.objectContaining({ search: `Research ${count - 1}`, cursor: undefined, limit: 50 }),
    );
    await actor.keyboard(count === 400 ? '{ArrowUp}{Enter}' : '{ArrowDown}{ArrowDown}{Enter}');
    const value = `sbx_${String(count - 1).padStart(32, '0')}`;
    expect(change).toHaveBeenLastCalledWith(value);
    view.rerender(
      <FilterOptionPicker api={api} kind="sandbox" value={value} onChange={change} active />,
    );
    const trigger = screen.getByRole('button', {
      name: `Sandbox: Research ${count - 1} · Sandbox reference`,
    });
    expect(trigger).toHaveFocus();
    expect(document.body.textContent).not.toContain(value);
    expect(trigger.title).not.toContain(value);
    await actor.click(trigger);
    await actor.click(screen.getByRole('menuitemradio', { name: 'All Sandboxes' }));
    expect(change).toHaveBeenLastCalledWith('');
    expect(api.create).not.toHaveBeenCalled();
    expect(api.report).not.toHaveBeenCalled();
  });
}
it('shows loading, failure/retry and empty choices without changing the selection', async () => {
  const actor = userEvent.setup();
  const api = metricsClient(true);
  let reject!: (error: Error) => void;
  api.options.mockImplementationOnce(
    () =>
      new Promise((_resolve, fail) => {
        reject = fail;
      }),
  );
  const change = vi.fn();
  render(<FilterOptionPicker api={api} kind="owner" value="" onChange={change} active />);
  await actor.click(screen.getByRole('button', { name: 'Owner: Everyone' }));
  await screen.findByText('Loading owner choices…');
  await act(async () => reject(new ApiError(503, 'Unavailable')));
  await screen.findByRole('alert');
  expect(screen.queryByText('No matching owner choices.')).not.toBeInTheDocument();
  api.options.mockResolvedValueOnce({ items: [], next: null });
  await actor.click(screen.getByRole('button', { name: 'Retry choices' }));
  await screen.findByText('No matching owner choices.');
  expect(change).not.toHaveBeenCalled();
});
it('resolves deep-linked labels, preserves duplicate names and never renders a missing selected ID', async () => {
  const api = metricsClient(true);
  api.options.mockResolvedValue({
    items: [owner, { value: 'second-owner', label: 'Alex Chen · account e8765432abcd' }],
    next: null,
  });
  const view = render(
    <FilterOptionPicker api={api} kind="owner" value={owner.value} onChange={() => {}} active />,
  );
  expect(document.body.textContent).not.toContain(owner.value);
  await screen.findByRole('button', { name: `Owner: ${owner.label}` });
  expect(api.options).toHaveBeenCalledWith(expect.objectContaining({ selected: owner.value }));
  await userEvent.click(screen.getByRole('button', { name: `Owner: ${owner.label}` }));
  await screen.findByRole('menuitemradio', { name: owner.label });
  expect(screen.getAllByRole('menuitemradio', { name: /Alex Chen/ })).toHaveLength(2);
  api.options.mockResolvedValue({ items: [], next: null });
  view.rerender(
    <FilterOptionPicker
      api={api}
      kind="owner"
      value="missing-user-id"
      onChange={() => {}}
      active
    />,
  );
  await screen.findByRole('button', { name: 'Owner: Owner unavailable' });
  expect(document.body.textContent).not.toContain('missing-user-id');
  await screen.findByText(/selected owner is no longer available/);
});
it('aborts a superseded search and ignores a late response', async () => {
  const api = metricsClient(true);
  let resolveOld!: (value: unknown) => void;
  let oldSignal!: AbortSignal;
  api.options.mockImplementationOnce((query: UsageFilterOptionsQuery) => {
    oldSignal = query.signal!;
    return new Promise((resolve) => {
      resolveOld = resolve;
    });
  });
  api.options.mockResolvedValue({ items: [owner], next: null });
  render(<FilterOptionPicker api={api} kind="owner" value="" onChange={() => {}} active />);
  await userEvent.click(screen.getByRole('button', { name: 'Owner: Everyone' }));
  fireEvent.change(screen.getByRole('textbox'), { target: { value: 'Alex' } });
  await screen.findByRole('menuitemradio', { name: owner.label });
  expect(oldSignal.aborted).toBe(true);
  await act(async () =>
    resolveOld({ items: [{ value: 'obsolete', label: 'Obsolete' }], next: null }),
  );
  expect(within(screen.getByRole('menu')).queryByText('Obsolete')).not.toBeInTheDocument();
  await userEvent.keyboard('{Escape}');
  await waitFor(() => expect(screen.queryByRole('menu')).not.toBeInTheDocument());
  expect(screen.getByRole('button', { name: 'Owner: Everyone' })).toHaveFocus();
});
