import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { StartupProgress } from './StartupProgress';
import { STAGES, stageProgress } from './progress';

/**
 * The bar must reflect steps that actually happened. A bar that advances on a
 * timer looks like progress and reports nothing, which costs the user their
 * trust in every other bar they see.
 */

describe('StartupProgress', () => {
  it('shows nothing complete before any step is reported', () => {
    render(<StartupProgress message="Starting" stage={null} />);
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '0');
  });

  it('advances with each reported step', () => {
    const seen = STAGES.map((stage) => {
      const { unmount } = render(<StartupProgress message="Starting" stage={stage} />);
      const value = Number(screen.getByRole('progressbar').getAttribute('aria-valuenow'));
      unmount();
      return value;
    });

    // Strictly increasing: two steps must never report the same progress, or
    // the bar stalls while work is happening.
    for (let i = 1; i < seen.length; i += 1) {
      expect(seen[i]).toBeGreaterThan(seen[i - 1] ?? 0);
    }
  });

  it('reaches complete only when the session is ready', () => {
    render(<StartupProgress message="Starting" stage="ready" />);
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '100');
  });

  it('names the step rather than only showing a bar', () => {
    // Which part is slow is most of what someone wants when it is slow.
    render(<StartupProgress message="Starting" stage="runtime_requested" />);
    expect(screen.getByText(/asking for a sandbox/i)).toBeInTheDocument();
  });

  it('keeps the message readable as ordinary text', () => {
    // The message is split per character for the animation; it must still be
    // one sentence to assistive technology.
    render(<StartupProgress message="Starting a sandbox" stage="accepted" />);
    expect(screen.getByText('Starting a sandbox')).toBeInTheDocument();
  });
});

describe('stageProgress', () => {
  it('maps the final step to fully complete', () => {
    expect(stageProgress('ready')).toBe(1);
  });

  it('never reports zero for a step that has happened', () => {
    // Something reported is something achieved; showing it as no progress
    // would be misleading.
    for (const stage of STAGES) {
      expect(stageProgress(stage)).toBeGreaterThan(0);
    }
  });
});
