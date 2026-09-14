import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { Sidebar } from './Sidebar';

it.each([true, false])(
  'preserves the animated SVG nodes across hierarchy refreshes and collapse changes with creation %s',
  (canCreateAgent) => {
    const draw = (collapsed: boolean, status: string) => (
      <Sidebar
        collapsed={collapsed}
        onToggleCollapsed={vi.fn()}
        account={<span>Account</span>}
        navigation={<span>{status}</span>}
        canCreateAgent={canCreateAgent}
        onNewAgent={vi.fn()}
      />
    );
    const { container, rerender } = render(draw(false, 'Active'));
    const mark = container.querySelector('.sidebar__mark svg');
    expect(mark).toBeInstanceOf(SVGElement);
    const animatedNodes = [
      ...mark!.querySelectorAll('.cma-arc, .cma-teeth, .cma-lens, .cma-bridge'),
    ];
    expect(animatedNodes).toHaveLength(5);
    for (const [collapsed, status] of [
      [false, 'Saving progress'],
      [true, 'Finishing save'],
      [false, 'Active'],
    ] as const) {
      rerender(draw(collapsed, status));
      expect(container.querySelector('.sidebar__mark svg')).toBe(mark);
      expect(mark).toBeInTheDocument();
      const currentNodes = [
        ...mark!.querySelectorAll('.cma-arc, .cma-teeth, .cma-lens, .cma-bridge'),
      ];
      currentNodes.forEach((node, index) => expect(node).toBe(animatedNodes[index]));
    }
    expect(screen.getByText('Active')).toBeInTheDocument();
  },
);

it('keeps home navigation, build badge and New agent as independent controls', async () => {
  const onNewAgent = vi.fn();
  const onHome = vi.fn();
  const actor = userEvent.setup();
  const { rerender } = render(
    <Sidebar
      collapsed={false}
      onToggleCollapsed={vi.fn()}
      account={<span>Account</span>}
      canCreateAgent
      onNewAgent={onNewAgent}
      onHome={onHome}
    />,
  );
  const home = screen.getByRole('link', { name: 'CMA home' });
  expect(home).not.toContainElement(screen.getByRole('button', { name: 'New agent' }));
  expect(home).not.toContainElement(screen.getByLabelText(/^Build /));
  await actor.click(home);
  expect(onHome).toHaveBeenCalledOnce();
  expect(onNewAgent).not.toHaveBeenCalled();
  await actor.click(screen.getByRole('button', { name: 'New agent' }));
  expect(onNewAgent).toHaveBeenCalledOnce();
  expect(onHome).toHaveBeenCalledOnce();

  rerender(
    <Sidebar collapsed={false} onToggleCollapsed={vi.fn()} account={<span>Account</span>} />,
  );
  expect(screen.getByRole('link', { name: 'CMA home' })).toHaveAttribute('href', '/');
});
