import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import type { MemoryPressure, Resources as ResourceReading } from './conversation';
import { MemoryPressureNotice, Resources, formatBytes } from './Resources';

function reading(overrides: Partial<ResourceReading> = {}): ResourceReading {
  return {
    cpuMillisUsed: undefined,
    cpuMillisLimit: undefined,
    memoryBytesUsed: undefined,
    memoryBytesLimit: undefined,
    memoryBytesPeak: undefined,
    stallPermille: undefined,
    ceilingHits: undefined,
    allocationFailures: undefined,
    kills: undefined,
    receivedBytes: undefined,
    sentBytes: undefined,
    receivedBytesPerSecond: undefined,
    sentBytesPerSecond: undefined,
    intervalMs: undefined,
    ...overrides,
  };
}

const PRESSURE: MemoryPressure = {
  usedBytes: 892 * 1024 ** 2,
  limitBytes: 1024 ** 3,
  occupancyPermille: 871,
  since: '2026-08-23T04:12:00Z',
};

describe('the seven resource sentences', () => {
  const cases = [
    {
      name: 'no grant',
      view: <Resources granted={false} />,
      expected: 'Resources not reported for this session',
    },
    {
      name: 'an older runtime',
      view: <Resources granted unreported />,
      expected: "This session's runtime cannot report what it is using",
    },
    {
      name: 'nothing arrived',
      view: <Resources granted />,
      expected: 'Resources · waiting',
    },
    {
      name: 'a bounded figure',
      view: (
        <Resources
          granted
          resources={reading({
            memoryBytesUsed: 892 * 1024 ** 2,
            memoryBytesLimit: 1024 ** 3,
          })}
        />
      ),
      expected: 'Memory · 892 MiB / 1 GiB',
    },
    {
      name: 'a figure without a ceiling',
      view: <Resources granted resources={reading({ memoryBytesUsed: 892 * 1024 ** 2 })} />,
      expected: 'Memory · 892 MiB, no ceiling reported',
    },
    {
      name: 'one absent figure',
      view: <Resources granted resources={reading({ receivedBytes: 18 * 1024 })} />,
      expected: 'Memory · not reported',
    },
    {
      name: 'the first CPU reading',
      view: <Resources granted resources={reading({ memoryBytesUsed: 892 * 1024 ** 2 })} />,
      expected: 'CPU · measuring',
    },
  ] as const;

  it.each(cases)('renders $name without inventing zero', ({ view, expected }) => {
    render(view);
    expect(screen.getByText(expected)).toBeInTheDocument();
    expect(expected).not.toContain('0');
  });

  it('keeps all seven meanings in different words', () => {
    expect(new Set(cases.map(({ expected }) => expected))).toHaveLength(7);
  });
});

describe('bounded and unbounded readings', () => {
  it('gives a bounded memory reading one accessible percentage track', () => {
    render(
      <Resources
        granted
        resources={reading({
          memoryBytesUsed: 892 * 1024 ** 2,
          memoryBytesLimit: 1024 ** 3,
        })}
      />,
    );

    const track = screen.getByRole('progressbar', { name: 'Memory' });
    expect(track).toHaveAttribute('aria-valuemin', '0');
    expect(track).toHaveAttribute('aria-valuemax', '100');
    expect(track).toHaveAttribute('aria-valuenow', '87');
    expect(screen.getAllByRole('progressbar')).toHaveLength(1);
  });

  it('drops the track when the memory ceiling is absent', () => {
    render(<Resources granted resources={reading({ memoryBytesUsed: 892 * 1024 ** 2 })} />);

    expect(screen.getByText('Memory · 892 MiB, no ceiling reported')).toBeInTheDocument();
    expect(screen.queryByRole('progressbar')).toBeNull();
  });

  it('renders CPU against its own ceiling and measures the first rate', () => {
    const view = render(
      <Resources granted resources={reading({ cpuMillisUsed: 500, cpuMillisLimit: 1000 })} />,
    );
    expect(screen.getByText('CPU · 500m / 1 core')).toBeInTheDocument();
    expect(screen.getByRole('progressbar', { name: 'CPU' })).toHaveAttribute('aria-valuenow', '50');

    view.rerender(<Resources granted resources={reading({ cpuMillisLimit: 1000 })} />);
    expect(screen.getByText('CPU · measuring')).toBeInTheDocument();
    expect(screen.queryByRole('progressbar')).toBeNull();
  });

  it('renders network as a figure pair and never gives it a track', () => {
    render(
      <Resources
        granted
        resources={reading({
          receivedBytes: 18 * 1024,
          sentBytes: Math.round(8.9 * 1024),
        })}
      />,
    );

    expect(screen.getByText('Network · ↓ 18 KiB · ↑ 8.9 KiB')).toBeInTheDocument();
    expect(screen.queryByRole('progressbar')).toBeNull();
  });

  it('states pressure in the memory figure and accessible label as well as colour', () => {
    render(
      <Resources
        granted
        resources={reading({
          memoryBytesUsed: PRESSURE.usedBytes,
          memoryBytesLimit: PRESSURE.limitBytes,
          cpuMillisUsed: 500,
          cpuMillisLimit: 1000,
        })}
        pressure={PRESSURE}
      />,
    );

    expect(screen.getByText('Memory under pressure · 892 MiB / 1 GiB')).toBeInTheDocument();
    expect(screen.getByRole('progressbar', { name: 'Memory under pressure' })).toHaveAttribute(
      'data-pressure',
      'true',
    );
    expect(screen.getByRole('progressbar', { name: 'CPU' })).not.toHaveAttribute('data-pressure');
  });
});

describe('byte figures', () => {
  it('groups below the unit boundary and changes precision with scale', () => {
    expect(formatBytes(892)).toBe('892 B');
    expect(formatBytes(8_900)).toBe('8.7 KiB');
    expect(formatBytes(18 * 1024)).toBe('18 KiB');
    expect(formatBytes(892 * 1024 ** 2)).toBe('892 MiB');
    expect(formatBytes(1024 ** 3)).toBe('1 GiB');
  });
});

describe('the memory-pressure notice', () => {
  it('is an actionable failure notice with figures and its opening time', () => {
    render(<MemoryPressureNotice pressure={PRESSURE} />);

    const alert = screen.getByRole('alert');
    expect(alert).toHaveClass('notice', 'notice--fail', 'chat__pressure');
    expect(alert).toHaveTextContent('892 MiB used of 1 GiB');
    expect(alert).toHaveTextContent('Opened at 23 Aug 2026, 12:12:00 SGT');
  });

  it('dismisses one episode and re-arms for a later opening time', async () => {
    const person = userEvent.setup();
    const view = render(<MemoryPressureNotice pressure={PRESSURE} />);

    await person.click(screen.getByRole('button', { name: 'Dismiss memory pressure notice' }));
    expect(screen.queryByRole('alert')).toBeNull();

    view.rerender(
      <MemoryPressureNotice pressure={{ ...PRESSURE, since: '2026-08-23T04:14:00Z' }} />,
    );
    expect(screen.getByRole('alert')).toHaveTextContent('Opened at 23 Aug 2026, 12:14:00 SGT');
  });

  it('renders no notice without an open episode', () => {
    const { container } = render(<MemoryPressureNotice pressure={undefined} />);
    expect(container.firstChild).toBeNull();
  });
});

describe('resource motion', () => {
  it('uses no keyframe or animation and only the shared transition vocabulary', () => {
    const css = readFileSync(resolve(process.cwd(), 'src/chat/Resources.css'), 'utf8');

    expect(css).not.toMatch(/@keyframes|\banimation(?:-name)?\s*:/);
    expect(css).not.toMatch(/cubic-bezier\(/);
    expect(css).toContain('transition: width var(--ds-transition-duration) var(--ds-ease-in-out);');
    expect(css).toContain(
      'transition: opacity var(--ds-transition-duration) var(--ds-ease-in-out);',
    );
    expect(css).toMatch(/prefers-reduced-motion:\s*reduce/);
  });
});
