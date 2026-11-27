import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { createRef } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { applyAppearance } from '../theme/appearance';
import { AgentMascot } from './AgentMascot';
import { agentMascotAssets } from './agentMascotAssets';
import { agentVisual, type AgentVisualAction } from './agentVisual';

afterEach(() => {
  document.body.removeAttribute('data-ds-dark-theme');
  document.documentElement.style.removeProperty('color-scheme');
  vi.unstubAllGlobals();
});

describe('AgentMascot', () => {
  const states = [
    ['rest', 'Agent paused'],
    ['ready', 'Ready'],
    ['idea', 'Thinking'],
    ['working', 'Working'],
    ['question', 'Waiting for approval'],
    ['love', 'Task complete'],
    ['preparing', 'Starting'],
    ['failed', 'Agent failed'],
  ] satisfies ReadonlyArray<readonly [AgentVisualAction, string]>;

  it.each(states)('shows the %s poster and visible status text', (action, label) => {
    applyAppearance('light');
    const { container } = render(<AgentMascot visual={{ action, label }} size={32} />);

    expect(screen.getByText(label)).toBeVisible();
    expect(screen.queryByRole('img')).toBeNull();
    expect(screen.queryByRole('status')).toBeNull();
    expect(screen.queryByRole('alert')).toBeNull();
    expect(container.querySelector('img')).toHaveAttribute(
      'src',
      agentMascotAssets[action].light.posters[32],
    );
    expect(container.querySelector('img')).toHaveAttribute('alt', '');
  });

  it.each([
    [16, 16, 24],
    [32, 32, 32],
    [48, 48, 48],
    [96, 48, 96],
  ] as const)(
    'renders size %s in a fixed slot using the appropriate poster',
    (size, source, slot) => {
      const ref = createRef<HTMLSpanElement>();
      const { container } = render(
        <AgentMascot visual={agentVisual({ status: 'working' })} size={size} ref={ref} />,
      );
      const image = container.querySelector('img');

      expect(ref.current).toHaveStyle({ inlineSize: `${slot}px`, blockSize: `${slot}px` });
      expect(ref.current).toHaveAttribute('aria-hidden', 'true');
      expect(image).toHaveAttribute('src', agentMascotAssets.working.light.posters[source]);
      expect(image).toHaveAttribute('width', String(size));
      expect(image).toHaveAttribute('height', String(size));
      expect(ref.current).toContainElement(image);
    },
  );

  it('follows the applied theme even when it differs from the device setting', async () => {
    vi.stubGlobal('matchMedia', () => ({ matches: true }));
    applyAppearance('light');
    const ref = createRef<HTMLSpanElement>();
    const visual = agentVisual({ status: 'working' });
    const { container } = render(<AgentMascot visual={visual} size={48} ref={ref} />);
    const slot = ref.current;

    expect(container.querySelector('img')).toHaveAttribute(
      'src',
      agentMascotAssets.working.light.posters[48],
    );

    await act(async () => applyAppearance('system'));
    await waitFor(() =>
      expect(container.querySelector('img')).toHaveAttribute(
        'src',
        agentMascotAssets.working.dark.posters[48],
      ),
    );
    expect(ref.current).toBe(slot);
  });

  it('keeps the text and slot when a poster fails and tries the next poster', () => {
    const ref = createRef<HTMLSpanElement>();
    const { rerender, container } = render(
      <AgentMascot visual={agentVisual({ status: 'working' })} size={16} ref={ref} />,
    );
    const slot = ref.current;
    const poster = container.querySelector('img');
    if (poster === null) throw new Error('Expected a working poster');
    fireEvent.error(poster);

    expect(screen.getByText('Working')).toBeVisible();
    expect(container.querySelector('img')).toBeNull();
    expect(slot?.querySelector('.agent-mascot__fallback')).toBeInTheDocument();

    rerender(<AgentMascot visual={agentVisual({ status: 'ready' })} size={16} ref={ref} />);

    expect(ref.current).toBe(slot);
    expect(screen.getByText('Ready')).toBeVisible();
    expect(container.querySelector('img')).toHaveAttribute(
      'src',
      agentMascotAssets.ready.light.posters[16],
    );
  });

  it('keeps the supplied unknown status text alongside the neutral poster', () => {
    const { container } = render(<AgentMascot visual={agentVisual({})} size={16} />);
    expect(screen.getByText('Status unavailable')).toBeVisible();
    expect(container.querySelector('img')).toHaveAttribute(
      'src',
      agentMascotAssets.rest.light.posters[16],
    );
  });
});
