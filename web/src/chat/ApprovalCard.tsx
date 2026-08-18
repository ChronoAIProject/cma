import type { Interrupt, InterruptOption } from '../agui';
import { Button } from '../ui/Button';
import './ApprovalCard.css';

/**
 * The card that appears when a session parks on `requires_action` —
 * `Web_Style.md` §18.3.
 *
 * The highest-stakes control in the product, and the one thing chat does that
 * a terminal genuinely cannot: in terminal mode approvals are keystrokes into
 * the kernel's own dialog, and `resolve_approval` on a terminal session is
 * refused by name. Here they are a structured request answered by id.
 *
 * **Not a modal, and this is binding.** A session can sit on an approval while
 * its owner is reading something else, and a modal would make every other
 * session in the rail unreachable until they answer. It sits in the message
 * stream where the turn stopped, and the rest of the interface stays usable.
 *
 * **It survives a reconnect**, because approvals ride the durable event stream
 * (`docs/API_AGUI.md` §4): a client that closes the tab and returns finds the
 * request waiting and answerable. Nothing here depends on having held a
 * connection — the card is drawn from the folded conversation like everything
 * else, and the conversation is folded from history as readily as from a live
 * stream.
 */

interface ApprovalCardProps {
  readonly interrupt: Interrupt;
  readonly onAnswer: (decision: string) => void;
  readonly busy?: boolean;
}

/**
 * What to offer when the kernel named no answers.
 *
 * The two the protocol always defines, and no more. §4 says a decision this
 * service does not recognise is read as a refusal, so inventing one would be
 * inventing a refusal — and a set this client made up would be a set no kernel
 * agreed to.
 */
const PROTOCOL_ANSWERS: readonly InterruptOption[] = [
  { decision: 'allow', label: 'Allow', affirmative: true },
  { decision: 'deny', label: 'Deny' },
];

export function ApprovalCard({ interrupt, onAnswer, busy = false }: ApprovalCardProps) {
  const answers =
    interrupt.options === undefined || interrupt.options.length === 0
      ? PROTOCOL_ANSWERS
      : interrupt.options;

  return (
    <section className="approval" aria-label="Approval requested">
      {/* The eyebrow — §18.3: approval, then the kernel's own reason. */}
      <p className="approval__eyebrow">approval · {interrupt.reason}</p>

      {/*
       * What the kernel wants, in the kernel's own words, as the largest type
       * on the surface — it is the sentence the decision turns on. Verbatim:
       * a kernel that says "Run `rm -rf build/`?" is asking about that
       * command, and rewording it into a category loses the one detail the
       * decision turns on.
       */}
      <p className="approval__question">{interrupt.message}</p>

      {/*
       * The answers the kernel offered, rendered from what arrived. A kernel
       * offering three renders three; this component has no opinion about how
       * many there are or what they are called. Beside them, the fact that
       * makes it safe to take a moment: nothing else is waiting on this.
       */}
      <div className="approval__answers">
        {answers.map((answer) => (
          <Button
            key={answer.decision}
            size="md"
            tone={answer.affirmative === true ? 'accent' : 'quiet'}
            disabled={busy}
            onClick={() => {
              onAnswer(answer.decision);
            }}
          >
            {answer.label ?? answer.decision}
          </Button>
        ))}
        <span className="approval__aside">Your other sessions keep working.</span>
      </div>
    </section>
  );
}
