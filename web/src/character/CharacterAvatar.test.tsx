import { render, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { CharacterAvatar, characterPhase } from './CharacterAvatar';
import { CharacterColorContext } from './characterColor';
import type { CharacterOptions } from './stage';

const stage = vi.hoisted(() => ({
  attach: vi.fn(),
}));

vi.mock('./stageBrowser', () => ({ characterStage: stage }));

afterEach(() => {
  stage.attach.mockReset();
  vi.restoreAllMocks();
});

function handle() {
  return { update: vi.fn<(options: CharacterOptions) => void>(), dispose: vi.fn<() => void>() };
}

describe('CharacterAvatar', () => {
  it('reserves its square and stays decorative before the stage is ready', () => {
    stage.attach.mockReturnValue(handle());
    const { container } = render(<CharacterAvatar pose="idle" size={24} />);
    const avatar = container.querySelector('.character-avatar');
    expect(avatar).toHaveAttribute('aria-hidden', 'true');
    expect(avatar).toHaveAttribute('data-pose', 'idle');
    expect(avatar).toHaveAttribute('data-motion', 'live');
    expect(avatar).toHaveStyle({ width: '24px', height: '24px' });
    expect(container.querySelector('canvas')).not.toBeNull();
  });

  it('attaches with its options and forwards later changes without remounting', async () => {
    const attached = handle();
    stage.attach.mockReturnValue(attached);
    const { container, rerender } = render(
      <CharacterAvatar pose="thinking" size={28} phase={1.5} />,
    );
    await waitFor(() => {
      expect(container.querySelector('.character-avatar')).toHaveAttribute('data-status', 'ready');
    });
    expect(stage.attach).toHaveBeenCalledWith(
      expect.any(HTMLCanvasElement),
      { pose: 'thinking', still: false, shadow: false, tone: 'brand', phase: 1.5 },
      expect.any(Function),
    );
    rerender(<CharacterAvatar pose="working" size={28} phase={1.5} tone="warning" />);
    expect(attached.update).toHaveBeenLastCalledWith({
      pose: 'working',
      still: false,
      shadow: false,
      tone: 'warning',
      phase: 1.5,
    });
    expect(stage.attach).toHaveBeenCalledTimes(1);
  });

  it('wears its Sandbox colour only in the brand tone, from a prop or its context', async () => {
    const attached = handle();
    stage.attach.mockReturnValue(attached);
    const tree = (avatar: ReactNode) => (
      <CharacterColorContext value="#2f96fa">{avatar}</CharacterColorContext>
    );
    const { container, rerender } = render(tree(<CharacterAvatar pose="idle" size={24} />));
    const avatar = container.querySelector<HTMLElement>('.character-avatar');
    await waitFor(() => {
      expect(avatar).toHaveAttribute('data-status', 'ready');
    });
    expect(stage.attach).toHaveBeenCalledWith(
      expect.any(HTMLCanvasElement),
      expect.objectContaining({ tone: 'brand', color: '#2f96fa' }),
      expect.any(Function),
    );
    expect(avatar?.style.color).toBe('rgb(47, 150, 250)');

    rerender(tree(<CharacterAvatar pose="idle" size={24} color="#62d079" />));
    expect(attached.update).toHaveBeenLastCalledWith(expect.objectContaining({ color: '#62d079' }));
    expect(avatar?.style.color).toBe('rgb(98, 208, 121)');

    rerender(tree(<CharacterAvatar pose="approval" size={24} tone="warning" color="#62d079" />));
    expect(attached.update).toHaveBeenLastCalledWith(
      expect.objectContaining({ tone: 'warning', color: undefined }),
    );
    expect(avatar?.style.color).toBe('');
  });

  it('shows the still mark when the stage cannot draw', async () => {
    stage.attach.mockReturnValue(undefined);
    const { container } = render(<CharacterAvatar pose="paused" size={24} tone="error" />);
    await waitFor(() => {
      expect(container.querySelector('img.character-avatar__fallback')).not.toBeNull();
    });
    expect(container.querySelector('.character-avatar')).toHaveAttribute('data-tone', 'error');
  });

  it('switches to the mark when the stage later loses its context', async () => {
    let lost: (() => void) | undefined;
    stage.attach.mockImplementation((_canvas: HTMLCanvasElement, _options, onLost: () => void) => {
      lost = onLost;
      return handle();
    });
    const { container } = render(<CharacterAvatar pose="idle" size={24} />);
    await waitFor(() => {
      expect(lost).toBeDefined();
    });
    lost?.();
    await waitFor(() => {
      expect(container.querySelector('.character-avatar__fallback')).not.toBeNull();
    });
  });

  it('logs and falls back when attaching throws', async () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    stage.attach.mockImplementation(() => {
      throw new Error('boom');
    });
    const { container } = render(<CharacterAvatar pose="idle" size={24} />);
    await waitFor(() => {
      expect(container.querySelector('.character-avatar__fallback')).not.toBeNull();
    });
    expect(error).toHaveBeenCalledWith(expect.stringContaining('[character]'), expect.any(Error));
  });

  it('releases its character on unmount', async () => {
    const attached = handle();
    stage.attach.mockReturnValue(attached);
    const { container, unmount } = render(<CharacterAvatar pose="idle" size={24} />);
    await waitFor(() => {
      expect(container.querySelector('.character-avatar')).toHaveAttribute('data-status', 'ready');
    });
    unmount();
    expect(attached.dispose).toHaveBeenCalledTimes(1);
  });

  it('does not attach once unmounted before the stage loads', async () => {
    stage.attach.mockReturnValue(handle());
    const { unmount } = render(<CharacterAvatar pose="idle" size={24} className="extra" />);
    unmount();
    await Promise.resolve();
    await Promise.resolve();
    expect(stage.attach).not.toHaveBeenCalled();
  });
});

describe('characterPhase', () => {
  it('is stable per identifier and stays inside one idle cycle', () => {
    expect(characterPhase('agt_1')).toBe(characterPhase('agt_1'));
    expect(characterPhase('agt_1')).not.toBe(characterPhase('agt_2'));
    for (const id of ['', 'a', 'agt_0123456789abcdef', 'x'.repeat(200)]) {
      const phase = characterPhase(id);
      expect(phase).toBeGreaterThanOrEqual(0);
      expect(phase).toBeLessThan(7);
    }
  });
});
