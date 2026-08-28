import { readFileSync } from 'node:fs';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import type { TriggerPreview } from '@chronoai/cma-sdk';
import { PublicTriggerPreview } from './PublicTriggerPreview';

const readTriggerPreview = vi.hoisted(() => vi.fn());

vi.mock('../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../api/client')>()),
  readTriggerPreview,
}));

const PREVIEW = JSON.parse(
  readFileSync('../sdk/typescript/tests/fixtures/trigger_preview.json', 'utf8'),
) as TriggerPreview;

beforeEach(() => {
  vi.clearAllMocks();
});

describe('public Trigger estimated cost', () => {
  it('states zero cost as exact', async () => {
    readTriggerPreview.mockResolvedValue(PREVIEW);
    render(<PublicTriggerPreview publicId={PREVIEW.public_id} onSignIn={vi.fn()} />);

    expect(await screen.findByText('$0.00')).toBeInTheDocument();
    expect(screen.getByText("Exact under this Trigger's zero-cost policy.")).toBeInTheDocument();
    expect(screen.queryByText(/About \$0\.00/)).toBeNull();
  });

  it('qualifies a contributor amount as provisional until authenticated review', async () => {
    readTriggerPreview.mockResolvedValue({
      ...PREVIEW,
      billing_mode: 'contributor',
      pricing_policy_reference: 'platform-rate-card-v1',
      estimated_cost: {
        kind: 'contributor_estimate',
        currency: 'USD',
        amount_micros: 25_000,
        display: '$0.025',
        quoted_at: '2026-08-26T12:00:00Z',
      },
    } satisfies TriggerPreview);
    render(<PublicTriggerPreview publicId={PREVIEW.public_id} onSignIn={vi.fn()} />);

    expect(await screen.findByText('About $0.025')).toBeInTheDocument();
    expect(
      screen.getByText(
        'Based on the defaults above. Authenticated review recalculates and finalizes the amount.',
      ),
    ).toBeInTheDocument();
  });

  it('does not invent an amount when the deployment cannot quote anonymously', async () => {
    readTriggerPreview.mockResolvedValue({
      ...PREVIEW,
      billing_mode: 'contributor',
      pricing_policy_reference: 'platform-rate-card-v1',
      estimated_cost: { kind: 'quote_unavailable' },
    } satisfies TriggerPreview);
    render(<PublicTriggerPreview publicId={PREVIEW.public_id} onSignIn={vi.fn()} />);

    expect(await screen.findByText('Estimate unavailable')).toBeInTheDocument();
    expect(
      screen.getByText(
        'Authenticated review will calculate the current estimate when deployment pricing is available.',
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/\$[0-9]/)).toBeNull();
  });
});
