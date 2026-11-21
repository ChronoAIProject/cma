import { fireEvent, render, screen } from '@testing-library/react';
import { useState } from 'react';
import { expect, it, vi } from 'vitest';
import { SandboxTabs } from './SandboxTabs';
import type { SandboxTab } from './sandboxTabRoute';

function Lazy({ onMount }: { readonly onMount: () => void }) {
  onMount();
  return <p>Activity content</p>;
}

function Harness({ onLazyMount = () => {} }: { readonly onLazyMount?: () => void }) {
  const [selected, setSelected] = useState<SandboxTab>('overview');
  return (
    <SandboxTabs
      label="Sandbox sections"
      selected={selected}
      onSelect={setSelected}
      tabs={[
        { id: 'overview', label: 'Overview', content: <p>Overview content</p> },
        { id: 'workspaces', label: 'Workspaces', content: <p>Workspaces content</p> },
        { id: 'access', label: 'Access', content: <input aria-label="Draft scope" /> },
        { id: 'activity', label: 'Activity', content: <Lazy onMount={onLazyMount} />, lazy: true },
      ]}
    />
  );
}

it('names the tablist and ties every tab to the panel it controls', () => {
  render(<Harness />);
  expect(screen.getByRole('tablist', { name: 'Sandbox sections' })).toBeInTheDocument();
  for (const tab of screen.getAllByRole('tab')) {
    const panel = document.getElementById(tab.getAttribute('aria-controls')!);
    expect(panel).toHaveAttribute('role', 'tabpanel');
    expect(panel).toHaveAttribute('aria-labelledby', tab.id);
  }
  expect(screen.getByRole('tab', { name: 'Overview' })).toHaveAttribute('aria-selected', 'true');
  expect(screen.getByRole('tabpanel', { name: 'Overview' })).toHaveTextContent('Overview content');
  expect(screen.getAllByRole('tabpanel')).toHaveLength(1);
});

it('moves and selects with arrow keys, wraps at the ends, and keeps one tab stop', () => {
  render(<Harness />);
  const tab = (name: string) => screen.getByRole('tab', { name });
  fireEvent.keyDown(tab('Overview'), { key: 'ArrowRight' });
  expect(tab('Workspaces')).toHaveAttribute('aria-selected', 'true');
  expect(tab('Workspaces')).toHaveFocus();
  expect(tab('Workspaces')).toHaveAttribute('tabindex', '0');
  expect(tab('Overview')).toHaveAttribute('tabindex', '-1');
  fireEvent.keyDown(tab('Workspaces'), { key: 'ArrowLeft' });
  fireEvent.keyDown(tab('Overview'), { key: 'ArrowLeft' });
  expect(tab('Activity')).toHaveAttribute('aria-selected', 'true');
  fireEvent.keyDown(tab('Activity'), { key: 'ArrowRight' });
  expect(tab('Overview')).toHaveFocus();
  fireEvent.keyDown(tab('Overview'), { key: 'End' });
  expect(tab('Activity')).toHaveFocus();
  fireEvent.keyDown(tab('Activity'), { key: 'Home' });
  expect(tab('Overview')).toHaveAttribute('aria-selected', 'true');
  fireEvent.keyDown(tab('Overview'), { key: 'Enter' });
  expect(tab('Overview')).toHaveAttribute('aria-selected', 'true');
});

it('keeps hidden panels mounted so unsaved input survives switching tabs', () => {
  render(<Harness />);
  fireEvent.click(screen.getByRole('tab', { name: 'Access' }));
  fireEvent.change(screen.getByRole('textbox', { name: 'Draft scope' }), {
    target: { value: 'github' },
  });
  fireEvent.click(screen.getByRole('tab', { name: 'Overview' }));
  expect(screen.queryByRole('textbox', { name: 'Draft scope' })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('tab', { name: 'Access' }));
  expect(screen.getByRole('textbox', { name: 'Draft scope' })).toHaveValue('github');
});

it('renders a lazy panel only once it is first opened, then keeps it', () => {
  const mounted = vi.fn();
  render(<Harness onLazyMount={mounted} />);
  expect(screen.queryByText('Activity content')).not.toBeInTheDocument();
  expect(mounted).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('tab', { name: 'Activity' }));
  expect(screen.getByText('Activity content')).toBeVisible();
  fireEvent.click(screen.getByRole('tab', { name: 'Overview' }));
  expect(screen.getByText('Activity content')).not.toBeVisible();
});
