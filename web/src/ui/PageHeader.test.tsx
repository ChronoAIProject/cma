import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { PageHeader } from './PageHeader';
import { MoreActions } from './MoreActions';

describe('PageHeader', () => {
  it('draws a breadcrumb of real links ending at the current page', async () => {
    const onSelect = vi.fn();
    const actor = userEvent.setup();
    render(
      <PageHeader
        breadcrumb={[
          { label: 'Agent Manager', href: '/agent-manager', onSelect },
          { label: 'Atlas', href: '/sandboxes/sbx_1' },
          { label: 'Atlas' },
        ]}
        title="Atlas"
      />,
    );
    const crumbs = screen.getByRole('navigation', { name: 'Breadcrumb' });
    const items = within(crumbs).getAllByRole('listitem');
    expect(items).toHaveLength(3);
    const home = within(crumbs).getByRole('link', { name: 'Agent Manager' });
    expect(home).toHaveAttribute('href', '/agent-manager');
    await actor.click(home);
    expect(onSelect).toHaveBeenCalledOnce();
    expect(items[2]?.querySelector('[aria-current="page"]')).toHaveTextContent('Atlas');
    expect(within(items[2]!).queryByRole('link')).toBeNull();
  });

  it('leaves a modified click on a crumb to the browser', () => {
    const onSelect = vi.fn();
    render(
      <PageHeader
        breadcrumb={[{ label: 'Docs', href: '/docs', onSelect }, { label: 'Protocol' }]}
        title="Protocol"
      />,
    );
    const link = screen.getByRole('link', { name: 'Docs' });
    const event = new MouseEvent('click', { bubbles: true, cancelable: true, metaKey: true });
    link.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
    expect(onSelect).not.toHaveBeenCalled();
  });

  it('follows a crumb without an onSelect as an ordinary link', () => {
    render(
      <PageHeader
        breadcrumb={[{ label: 'Docs', href: '/docs' }, { label: 'Here' }]}
        title="Here"
      />,
    );
    const link = screen.getByRole('link', { name: 'Docs' });
    const event = new MouseEvent('click', { bubbles: true, cancelable: true });
    link.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
  });

  it('puts the title, status, facts, description and actions in their places', () => {
    render(
      <PageHeader
        title="Prove lemma"
        headingId="title-id"
        badge={<span>Working</span>}
        meta={<span>agent/one</span>}
        description="Created manually"
        actions={<button type="button">Stop</button>}
      />,
    );
    const heading = screen.getByRole('heading', { level: 1, name: 'Prove lemma' });
    expect(heading).toHaveAttribute('id', 'title-id');
    expect(heading).toHaveAttribute('tabindex', '-1');
    expect(screen.getByText('Working').closest('.page-header__badge')).not.toBeNull();
    expect(screen.getByText('agent/one').closest('.page-header__meta')).not.toBeNull();
    expect(screen.getByText('Created manually')).toHaveClass('page-header__description');
    expect(screen.getByRole('button', { name: 'Stop' }).parentElement).toHaveClass(
      'page-header__actions',
    );
  });

  it('draws no breadcrumb, description or actions when there are none', () => {
    const { container } = render(<PageHeader title="Docs" />);
    expect(screen.queryByRole('navigation')).toBeNull();
    expect(container.querySelector('.page-header__description')).toBeNull();
    expect(container.querySelector('.page-header__actions')).toBeNull();
  });
});

describe('MoreActions', () => {
  it('lists the overflow with reasons and reports the chosen row', async () => {
    const onSelect = vi.fn();
    const actor = userEvent.setup();
    render(
      <MoreActions
        label="More Agent actions"
        items={[
          { id: 'stop', label: 'Stop agent' },
          { id: 'delete', label: 'Delete agent', disabled: true, reason: 'Stop it first.' },
        ]}
        onSelect={onSelect}
      />,
    );
    const trigger = screen.getByRole('button', { name: 'More Agent actions' });
    expect(trigger).toHaveAttribute('aria-expanded', 'false');
    await actor.click(trigger);
    expect(trigger).toHaveAttribute('aria-expanded', 'true');
    const remove = screen.getByRole('menuitem', { name: 'Delete agent Stop it first.' });
    await actor.click(remove);
    expect(onSelect).not.toHaveBeenCalled();
    await actor.click(screen.getByRole('menuitem', { name: 'Stop agent' }));
    expect(onSelect).toHaveBeenCalledWith('stop');
    expect(screen.queryByRole('menu')).toBeNull();
  });

  it('draws nothing when there is nothing to offer', () => {
    const { container } = render(<MoreActions items={[]} onSelect={vi.fn()} />);
    expect(container).toBeEmptyDOMElement();
  });
});
