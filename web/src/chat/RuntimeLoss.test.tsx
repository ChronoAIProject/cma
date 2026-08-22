import { useState } from 'react';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { RuntimeLoss } from './RuntimeLoss';
import type { RuntimeLoss as RuntimeLossState } from './conversation';

function lost(cause?: string, noticedAt?: string): RuntimeLossState {
  return {
    runtime: 'runtime-gone',
    seq: 12,
    ...(cause === undefined ? {} : { cause }),
    ...(noticedAt === undefined ? {} : { noticedAt }),
  };
}

const HEADLINES = [
  ['out_of_memory', "This session's runtime ran out of memory and was stopped."],
  ['failed', "This session's runtime stopped with an error."],
  ['displaced', 'The machine running this session was taken away.'],
  ['ended', "This session's runtime exited."],
  ['unrecognised', "This session's runtime stopped, for a reason this build does not recognise."],
  [undefined, "This session's runtime stopped. Nothing said why."],
] as const;

describe('the reason a runtime was lost', () => {
  it.each(HEADLINES)('renders %s as its own exact headline', (cause, expected) => {
    render(<RuntimeLoss loss={lost(cause)} onDismiss={() => {}} />);

    expect(screen.getByText(expected)).toBeInTheDocument();
  });

  it('keeps all six meanings distinct', () => {
    expect(new Set(HEADLINES.map(([, expected]) => expected))).toHaveLength(6);
  });
});

describe('the runtime-loss notice', () => {
  it('renders nothing when there was no loss', () => {
    const { container } = render(<RuntimeLoss loss={undefined} onDismiss={() => {}} />);

    expect(container.firstChild).toBeNull();
  });

  it('says when the service noticed, in the product display zone', () => {
    render(<RuntimeLoss loss={lost('ended', '2026-08-23T04:12:00Z')} onDismiss={() => {}} />);

    expect(screen.getByRole('status')).toHaveTextContent(
      'Noticed at 23 Aug 2026, 12:12:00 SGT. A new runtime was started — the agent does not carry a conversation across a runtime it lost, so it begins again from what was captured.',
    );
  });

  it('omits the first sentence when no noticed time exists', () => {
    render(<RuntimeLoss loss={lost('ended')} onDismiss={() => {}} />);

    const notice = screen.getByRole('status');
    expect(notice).toHaveTextContent(
      'A new runtime was started — the agent does not carry a conversation across a runtime it lost, so it begins again from what was captured.',
    );
    expect(notice).not.toHaveTextContent('Noticed at');
    expect(notice).not.toHaveTextContent('undefined');
    expect(notice).not.toHaveTextContent('0');
  });

  it('is quiet status history and never a failure alert', () => {
    render(<RuntimeLoss loss={lost('failed')} onDismiss={() => {}} />);

    const notice = screen.getByRole('status');
    expect(notice).toHaveClass('notice', 'notice--quiet', 'conversation__loss');
    expect(notice).not.toHaveClass('notice--fail');
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('can be dismissed', async () => {
    const person = userEvent.setup();

    function Dismissible() {
      const [dismissed, setDismissed] = useState(false);
      return (
        <RuntimeLoss
          loss={dismissed ? undefined : lost('displaced')}
          onDismiss={() => {
            setDismissed(true);
          }}
        />
      );
    }

    render(<Dismissible />);
    await person.click(screen.getByRole('button', { name: 'Dismiss' }));

    expect(screen.queryByRole('status')).toBeNull();
  });
});
