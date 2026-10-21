import { createRef } from 'react';
import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { AgentHeader } from './AgentHeader';
import { storedHeaderPin } from './headerPin';

afterEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
});

function draw(overrides: Partial<Parameters<typeof AgentHeader>[0]> = {}) {
  return (
    <>
      <AgentHeader
        title="Prove lemma 4.2"
        status="ready"
        viewerId="alice"
        breadcrumb={[
          { label: 'research', href: '/sandboxes/sbx_1' },
          { label: 'trureturing', href: '/workspaces/wsp_1' },
          { label: 'Prove lemma 4.2' },
        ]}
        branch="cma/proof"
        detailsOpen={false}
        onDetailsOpenChange={vi.fn()}
        {...overrides}
      />
      <textarea aria-label="Message" />
    </>
  );
}

const seat = () => document.querySelector('.agent-header-seat')!;

it('starts collapsed to a strip naming the Agent, with the full header still in the document', () => {
  render(draw());
  expect(seat()).not.toHaveAttribute('data-pinned');
  expect(seat()).not.toHaveAttribute('data-revealed');
  expect(seat().querySelector('.agent-header__strip')).toHaveTextContent('Prove lemma 4.2');
  expect(screen.getByRole('heading', { level: 1, name: 'Prove lemma 4.2' })).toBeInTheDocument();
  expect(screen.getByRole('link', { name: 'research' })).toHaveAttribute(
    'href',
    '/sandboxes/sbx_1',
  );
  expect(screen.getByText('cma/proof')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Pin header' })).toHaveAttribute(
    'aria-pressed',
    'false',
  );
});

it('reveals on hover and on keyboard focus, and hides again when both leave', async () => {
  const user = userEvent.setup();
  render(draw());
  fireEvent.pointerEnter(seat());
  expect(seat()).toHaveAttribute('data-revealed');
  fireEvent.pointerLeave(seat());
  expect(seat()).not.toHaveAttribute('data-revealed');
  // Tab reaches the first breadcrumb link while collapsed, which reveals the header.
  await user.tab();
  expect(screen.getByRole('link', { name: 'research' })).toHaveFocus();
  expect(seat()).toHaveAttribute('data-revealed');
  await user.click(screen.getByRole('textbox', { name: 'Message' }));
  expect(seat()).not.toHaveAttribute('data-revealed');
});

it('keeps the heading focusable for focus-on-open, which reveals the header', () => {
  const heading = createRef<HTMLHeadingElement>();
  render(draw({ headingRef: heading }));
  act(() => heading.current!.focus());
  expect(heading.current).toHaveFocus();
  expect(heading.current).toHaveAttribute('tabindex', '-1');
  expect(seat()).toHaveAttribute('data-revealed');
});

it('stays revealed while its menu is open, though the list is drawn on the body', async () => {
  const user = userEvent.setup();
  render(draw());
  await user.click(screen.getByRole('button', { name: 'More Agent actions' }));
  expect(screen.getByRole('menuitem', { name: 'Show Agent details' })).toHaveFocus();
  expect(seat()).toHaveAttribute('data-revealed');
});

it('remembers the pin per viewer and restores it on the next visit', async () => {
  const user = userEvent.setup();
  const view = render(draw());
  await user.click(screen.getByRole('button', { name: 'Pin header' }));
  expect(seat()).toHaveAttribute('data-pinned');
  expect(seat().querySelector('.agent-header__strip')).toBeNull();
  expect(screen.getByRole('button', { name: 'Unpin header' })).toHaveAttribute(
    'aria-pressed',
    'true',
  );
  expect(storedHeaderPin('alice')).toBe(true);
  expect(storedHeaderPin('bob')).toBe(false);
  view.unmount();
  render(draw());
  expect(seat()).toHaveAttribute('data-pinned');
  await user.click(screen.getByRole('button', { name: 'Unpin header' }));
  expect(seat()).not.toHaveAttribute('data-pinned');
  expect(storedHeaderPin('alice')).toBe(false);
});

it('treats unreadable storage as unpinned and still pins for this view when writes fail', async () => {
  const user = userEvent.setup();
  vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
    throw new DOMException('Blocked', 'SecurityError');
  });
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new DOMException('Full', 'QuotaExceededError');
  });
  render(draw());
  expect(seat()).not.toHaveAttribute('data-pinned');
  await user.click(screen.getByRole('button', { name: 'Pin header' }));
  expect(seat()).toHaveAttribute('data-pinned');
});

it('starts its menu with the Details toggle, then the origin rows, then the actions', async () => {
  const user = userEvent.setup();
  const onDetailsOpenChange = vi.fn();
  const onMenuSelect = vi.fn();
  const onMoreAction = vi.fn();
  const assign = vi.fn();
  vi.spyOn(window, 'location', 'get').mockReturnValue({ ...window.location, assign });
  render(
    draw({
      onDetailsOpenChange,
      onMenuSelect,
      onMoreAction,
      menu: [
        { type: 'label', id: 'creation-origin', text: 'Created by Trigger' },
        { id: 'published-trigger', label: 'Published Trigger: Intro', href: '/t/pub_1' },
        { id: 'subscription-sign-in', label: 'Sign in again' },
      ],
      moreActions: [{ id: 'stop_agent', label: 'Stop agent' }],
    }),
  );
  const open = () => user.click(screen.getByRole('button', { name: 'More Agent actions' }));
  await open();
  expect(screen.getAllByRole('menuitem').map((row) => row.textContent)).toEqual([
    'Show Agent details',
    'Published Trigger: Intro',
    'Sign in again',
    'Stop agent',
  ]);
  expect(screen.getByRole('menu')).toHaveTextContent('Created by Trigger');
  await user.click(screen.getByRole('menuitem', { name: 'Show Agent details' }));
  expect(onDetailsOpenChange).toHaveBeenCalledWith(true);
  await open();
  await user.click(screen.getByRole('menuitem', { name: 'Published Trigger: Intro' }));
  expect(assign).toHaveBeenCalledWith('/t/pub_1');
  await open();
  await user.click(screen.getByRole('menuitem', { name: 'Sign in again' }));
  expect(onMenuSelect).toHaveBeenCalledWith('subscription-sign-in');
  await open();
  await user.click(screen.getByRole('menuitem', { name: 'Stop agent' }));
  expect(onMoreAction).toHaveBeenCalledWith('stop_agent');
});

it('names the open state and holds the Details row with its reason while access is uncertain', async () => {
  const user = userEvent.setup();
  const onDetailsOpenChange = vi.fn();
  const { rerender } = render(draw({ detailsOpen: true, onDetailsOpenChange }));
  await user.click(screen.getByRole('button', { name: 'More Agent actions' }));
  await user.click(screen.getByRole('menuitem', { name: 'Hide Agent details' }));
  expect(onDetailsOpenChange).toHaveBeenCalledWith(false);
  rerender(draw({ detailsUnavailable: true, onDetailsOpenChange }));
  await user.click(screen.getByRole('button', { name: 'More Agent actions' }));
  const row = screen.getByRole('menuitem', { name: /^Show Agent details/ });
  expect(row).toHaveAttribute('aria-disabled', 'true');
  expect(row).toHaveTextContent('Details are available once access is confirmed.');
  await user.click(row);
  expect(onDetailsOpenChange).toHaveBeenCalledTimes(1);
});

it('lists actions only where they can be carried out', async () => {
  const user = userEvent.setup();
  render(draw({ moreActions: [{ id: 'stop_agent', label: 'Stop agent' }] }));
  await user.click(screen.getByRole('button', { name: 'More Agent actions' }));
  expect(screen.queryByRole('menuitem', { name: 'Stop agent' })).not.toBeInTheDocument();
});
