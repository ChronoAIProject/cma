import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ApiError, type UsageFilterOptionsQuery } from '@chronoai/cma-sdk';
import { expect, it, vi } from 'vitest';
import { FilterOptionPicker } from './FilterOptionPicker';
import { metricsClient } from './testFixtures';

const owner = {
  value: '91892afa-5fa5-4f4f-9100-2b8d986d2f71',
  label: 'Alex Chen',
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
            label: `Research ${start + index}`,
          }),
        ),
        next: start + 50 < count ? String(start + 50) : null,
      };
    });
    const view = render(
      <FilterOptionPicker api={api} kind="sandbox" value="" onChange={change} active />,
    );
    await actor.click(screen.getByRole('button', { name: 'Sandbox: All Sandboxes' }));
    await screen.findByRole('menuitemradio', { name: 'Research 0' });
    expect(screen.getAllByRole('menuitemradio')).toHaveLength(51);
    await actor.click(screen.getByRole('button', { name: 'Next choices' }));
    await screen.findByRole('menuitemradio', { name: 'Research 50' });
    expect(screen.getAllByRole('menuitemradio')).toHaveLength(51);
    const search = screen.getByRole('textbox', { name: 'Search sandbox choices' });
    await actor.click(search);
    await actor.type(search, `Research ${count - 1}`);
    await screen.findByRole('menuitemradio', { name: `Research ${count - 1}` });
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
      name: `Sandbox: Research ${count - 1}`,
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
    items: [
      { ...owner, disambiguator: 'Joined 2026-11-01' },
      { value: 'second-owner', label: 'Alex Chen', disambiguator: 'Joined 2026-11-02' },
    ],
    next: null,
  });
  const view = render(
    <FilterOptionPicker api={api} kind="owner" value={owner.value} onChange={() => {}} active />,
  );
  expect(document.body.textContent).not.toContain(owner.value);
  await screen.findByRole('button', { name: `Owner: ${owner.label}` });
  expect(api.options).toHaveBeenCalledWith(expect.objectContaining({ selected: owner.value }));
  await userEvent.click(screen.getByRole('button', { name: `Owner: ${owner.label}` }));
  await screen.findByRole('menuitemradio', { name: 'Alex Chen Joined 2026-11-01' });
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

for (const kind of ['owner', 'sandbox'] as const) {
  it(`${kind} shows duplicate context only in the current list and keeps selections plain`, async () => {
    const api = metricsClient(true);
    const change = vi.fn();
    const name = kind === 'owner' ? 'Alex Chen' : 'Research';
    const context = kind === 'owner' ? 'Joined 2026-11-01' : 'Alex Chen';
    const otherContext = kind === 'owner' ? 'Joined 2026-11-02' : 'Priya';
    const item = { value: 'opaque-first', label: name, disambiguator: context };
    api.options.mockResolvedValue({
      items: [item, { value: 'opaque-second', label: name, disambiguator: otherContext }],
      next: null,
    });
    const label = kind === 'owner' ? 'Owner' : 'Sandbox';
    const view = render(
      <FilterOptionPicker api={api} kind={kind} value="" onChange={change} active />,
    );
    await userEvent.click(screen.getByRole('button', { name: new RegExp(`^${label}:`) }));
    await userEvent.click(await screen.findByRole('menuitemradio', { name: `${name} ${context}` }));
    expect(change).toHaveBeenLastCalledWith(item.value);
    view.rerender(
      <FilterOptionPicker api={api} kind={kind} value={item.value} onChange={change} active />,
    );
    const trigger = screen.getByRole('button', { name: `${label}: ${name}` });
    expect(trigger.title).toBe(name);
    expect(trigger).not.toHaveTextContent(context);
    api.options.mockResolvedValue({ items: [item], next: null });
    await userEvent.click(trigger);
    await screen.findByRole('menuitemradio', { name });
    expect(screen.queryByText(context, { exact: true })).not.toBeInTheDocument();
  });
}

it('keeps a last-resort duplicate reference out of the selected label and title', async () => {
  const api = metricsClient(true);
  const change = vi.fn();
  const item = { value: 'first', label: 'Unnamed owner', disambiguator: 'reference 123456abcdef' };
  api.options.mockResolvedValue({
    items: [
      item,
      { value: 'second', label: 'Unnamed owner', disambiguator: 'reference abcdef123456' },
    ],
    next: null,
  });
  const view = render(
    <FilterOptionPicker api={api} kind="owner" value="" onChange={change} active />,
  );
  await userEvent.click(screen.getByRole('button', { name: 'Owner: Everyone' }));
  await userEvent.click(
    await screen.findByRole('menuitemradio', { name: 'Unnamed owner reference 123456abcdef' }),
  );
  view.rerender(
    <FilterOptionPicker api={api} kind="owner" value="first" onChange={change} active />,
  );
  const selected = screen.getByRole('button', { name: 'Owner: Unnamed owner' });
  expect(selected.title).toBe('Unnamed owner');
  expect(selected).not.toHaveTextContent('reference');
});

for (const [kind, label, safe] of [
  ['owner', 'Owner', 'Unnamed owner'],
  ['sandbox', 'Sandbox', 'Unnamed Sandbox'],
  ['runtime', 'Runtime version', 'Unnamed runtime'],
] as const) {
  it(`${kind} selected resolution keeps UUIDs out of text, accessible name and title`, async () => {
    const api = metricsClient(true);
    const value = 'Research-91892afa-5fa5-4f4f-9100-2b8d986d2f71';
    api.options.mockResolvedValue({ items: [{ value, label: safe }], next: null });
    render(<FilterOptionPicker api={api} kind={kind} value={value} onChange={() => {}} active />);
    const selected = await screen.findByRole('button', { name: `${label}: ${safe}` });
    expect(selected).toHaveTextContent(safe);
    expect(selected.title).toBe(safe);
    expect(selected.getAttribute('aria-label')).toBe(`${label}: ${safe}`);
    expect(document.body.textContent).not.toContain(value);
    expect(api.options).toHaveBeenCalledWith(expect.objectContaining({ selected: value }));
  });
}

for (const [kind, contexts] of [
  ['sandbox', ['Alice · Created 2026-11-01', 'Alice · Created 2026-11-02']],
  ['sandbox', ['Alice · Created 2026-11-01 12:30 UTC', 'Alice · Created 2026-11-01 12:31 UTC']],
  [
    'sandbox',
    ['Alice · Created 2026-11-01 12:30:00 UTC', 'Alice · Created 2026-11-01 12:30:01 UTC'],
  ],
  ['owner', ['Joined 2026-11-01 12:30 UTC', 'Joined 2026-11-01 12:31 UTC']],
  ['owner', ['Joined 2026-11-01 12:30:00.001 UTC', 'Joined 2026-11-01 12:30:00.002 UTC']],
] as const) {
  it(`${kind} announces distinct human context for ${contexts[0]} and keeps the selected name plain`, async () => {
    const api = metricsClient(true);
    const change = vi.fn();
    const name = kind === 'owner' ? 'Alice' : 'Research';
    const label = kind === 'owner' ? 'Owner' : 'Sandbox';
    const items = contexts.map((disambiguator, index) => ({
      value: `opaque-${index}`,
      label: name,
      disambiguator,
    }));
    api.options.mockResolvedValue({ items, next: null });
    const view = render(
      <FilterOptionPicker api={api} kind={kind} value="" onChange={change} active />,
    );
    await userEvent.click(screen.getByRole('button', { name: new RegExp(`^${label}:`) }));
    for (const context of contexts) {
      const option = await screen.findByRole('menuitemradio', { name: `${name} ${context}` });
      expect(option).toHaveTextContent(context);
      expect(option).not.toHaveTextContent(/reference|opaque/);
    }
    await userEvent.click(screen.getByRole('menuitemradio', { name: `${name} ${contexts[1]}` }));
    expect(change).toHaveBeenLastCalledWith('opaque-1');
    view.rerender(
      <FilterOptionPicker api={api} kind={kind} value="opaque-1" onChange={change} active />,
    );
    const trigger = screen.getByRole('button', { name: `${label}: ${name}` });
    expect(trigger).toHaveTextContent(name);
    expect(trigger.title).toBe(name);
    expect(trigger).not.toHaveTextContent(/Created|Joined|reference|opaque/);
  });
}

it('announces distinct final contexts when an owner name resembles generated creation text', async () => {
  const api = metricsClient(true);
  const change = vi.fn();
  const contexts = [
    'Alice · Created 2026-11-01 12:30 UTC',
    'Alice · Created 2026-11-02',
    'Alice · Created 2026-11-01 · Created 2026-11-03',
  ];
  api.options.mockResolvedValue({
    items: contexts.map((disambiguator, index) => ({
      value: `sandbox-${index}`,
      label: 'Research',
      disambiguator,
    })),
    next: null,
  });
  const view = render(
    <FilterOptionPicker api={api} kind="sandbox" value="" onChange={change} active />,
  );
  await userEvent.click(screen.getByRole('button', { name: 'Sandbox: All Sandboxes' }));
  for (const context of contexts) {
    const option = await screen.findByRole('menuitemradio', {
      name: `Research ${context}`,
    });
    expect(option).toHaveTextContent(context);
    expect(option).not.toHaveTextContent('reference');
  }
  expect(screen.getAllByRole('menuitemradio', { name: /^Research / })).toHaveLength(3);
  await userEvent.click(screen.getByRole('menuitemradio', { name: `Research ${contexts[2]}` }));
  expect(change).toHaveBeenLastCalledWith('sandbox-2');
  view.rerender(
    <FilterOptionPicker api={api} kind="sandbox" value="sandbox-2" onChange={change} active />,
  );
  const selected = screen.getByRole('button', { name: 'Sandbox: Research' });
  expect(selected.title).toBe('Research');
  expect(selected).not.toHaveTextContent(/Created|reference/);
});
