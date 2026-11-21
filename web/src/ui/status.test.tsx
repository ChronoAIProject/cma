import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { Alert } from './Alert';
import { Badge, StatusBadge } from './Badge';
import { ShellLoading, Skeleton, SkeletonRows } from './Skeleton';
import { statusLabel, statusTone } from '../vocabulary/status';
import { sandboxTone } from '../sandbox/lifecycle';
import { sandboxFixture } from '../test/hierarchy';

const STATUSES = [
  'provisioning',
  'installing',
  'active',
  'pausing',
  'paused',
  'reviving',
  'credential_rotating',
  'rotating',
  'failed',
  'retiring',
  'retired',
  'preparing',
  'ready',
  'removing',
  'removed',
  'creating',
  'starting',
  'working',
  'waiting_approval',
  'stopped',
  'deleted',
] as const;

describe('statusTone', () => {
  it.each([
    ['working', 'active'],
    ['starting', 'active'],
    ['pausing', 'active'],
    ['reviving', 'active'],
    ['rotating', 'active'],
    ['credential_rotating', 'active'],
    ['active', 'neutral'],
    ['ready', 'neutral'],
    ['waiting_approval', 'warning'],
    ['failed', 'error'],
    ['paused', 'muted'],
    ['stopped', 'muted'],
    ['deleted', 'muted'],
  ] as const)('draws %s as %s', (status, tone) => {
    expect(statusTone(status)).toBe(tone);
  });

  it('gives the same word the same tone wherever it comes from', () => {
    const toneByWord = new Map<string, string>();
    for (const status of STATUSES) {
      const word = statusLabel(status);
      const tone = statusTone(status);
      expect(toneByWord.get(word) ?? tone, word).toBe(tone);
      toneByWord.set(word, tone);
    }
  });
});

describe('sandboxTone', () => {
  it('is active while a lifecycle operation is in flight', () => {
    const sandbox = sandboxFixture();
    expect(sandboxTone(sandbox)).toBe('neutral');
    expect(
      sandboxTone({
        ...sandbox,
        lifecycle: {
          ...sandbox.lifecycle!,
          operation: { id: 'op', purpose: 'pause', startedAt: '2026-10-21T00:00:00Z' },
        } as never,
      }),
    ).toBe('active');
    expect(sandboxTone(sandboxFixture('paused'))).toBe('muted');
    expect(sandboxTone(sandboxFixture('failed'))).toBe('error');
    expect(sandboxTone(sandboxFixture('retiring'))).toBe('active');
  });
});

describe('Badge', () => {
  it.each(STATUSES)('pairs the %s word with the dot its tone draws', (status) => {
    const { container } = render(<StatusBadge status={status} />);
    const badge = container.querySelector('.badge');
    expect(badge).toHaveTextContent(statusLabel(status));
    expect(badge).toHaveAttribute('data-tone', statusTone(status));
    const dot = badge?.querySelector('.statedot');
    if (statusTone(status) === 'muted') expect(dot).toBeNull();
    else expect(dot).toHaveAttribute('aria-hidden', 'true');
  });

  it('maps tones to dots: active pings, neutral is quiet, warning and error colour', () => {
    const { container } = render(
      <>
        <Badge tone="active">a</Badge>
        <Badge tone="neutral">b</Badge>
        <Badge tone="warning">c</Badge>
        <Badge tone="error">d</Badge>
      </>,
    );
    expect(
      [...container.querySelectorAll('.statedot')].map((dot) => dot.getAttribute('data-state')),
    ).toEqual(['ongoing', 'neutral', 'warning', 'error']);
  });

  it('announces politely only when asked to', () => {
    const { rerender } = render(<StatusBadge status="working" />);
    expect(screen.queryByRole('status')).toBeNull();
    rerender(<StatusBadge status="working" live />);
    expect(screen.getByRole('status')).toHaveTextContent('Working');
  });
});

describe('Alert', () => {
  it.each([
    ['info', 'status'],
    ['success', 'status'],
    ['warning', 'alert'],
    ['error', 'alert'],
  ] as const)('gives %s the %s role', (tone, role) => {
    render(<Alert tone={tone}>Something happened.</Alert>);
    expect(screen.getByRole(role)).toHaveAttribute('data-tone', tone);
    expect(screen.getByRole(role)).toHaveTextContent('Something happened.');
  });

  it('draws a title, the words and actions, and forwards attributes a parent adds', () => {
    render(
      <Alert tone="warning" title="Pausing" actions={<button type="button">Retry</button>} inert>
        Messages cannot be sent while the Sandbox is changing state.
      </Alert>,
    );
    const alert = screen.getByText('Pausing').closest('.alert');
    expect(alert).toHaveAttribute('inert');
    expect(alert?.querySelector('.alert__body')).toHaveTextContent(
      'Messages cannot be sent while the Sandbox is changing state.',
    );
    expect(alert?.querySelector('.alert__actions button')).toHaveTextContent('Retry');
  });

  it('draws no empty body or action row', () => {
    const { container } = render(<Alert tone="info" title="Only a title" />);
    expect(container.querySelector('.alert__body')).toBeNull();
    expect(container.querySelector('.alert__actions')).toBeNull();
  });
});

describe('Skeleton', () => {
  it('is decorative and reserves the size it is given', () => {
    const { container } = render(<Skeleton width={120} height={20} radius={8} />);
    const block = container.querySelector('.skeleton');
    expect(block).toHaveAttribute('aria-hidden', 'true');
    expect(block).toHaveStyle({ width: '120px', height: '20px', borderRadius: '8px' });
  });

  it('gives a loading list one polite status named for what is loading', () => {
    const { container } = render(<SkeletonRows rows={3} label="Loading Agents" />);
    expect(screen.getByRole('status', { name: 'Loading Agents' })).toBeInTheDocument();
    expect(container.querySelectorAll('.skeleton-rows__row')).toHaveLength(3);
  });

  it('outlines the console while identity is checked', () => {
    const { container } = render(<ShellLoading label="Signing you in" />);
    expect(screen.getByRole('status', { name: 'Signing you in' })).toBeInTheDocument();
    expect(container.querySelector('.shell-loading__rail')).not.toBeNull();
    expect(container.querySelector('.landing__character')).toBeNull();
  });
});
