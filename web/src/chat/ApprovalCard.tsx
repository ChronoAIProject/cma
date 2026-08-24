import { useState } from 'react';
import type { InterruptOption } from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import { renderInline } from './inline';
import { parse } from './markdown';
import type { PendingApproval } from '@chronoai/cma-sdk';
import { timeOfDay } from '../sessions/sessionTime';
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
  readonly interrupt: PendingApproval;
  /**
   * Send the answer, and say whether the service took it.
   *
   * Returning the outcome is what lets the card put the question back when a
   * send is refused, instead of standing there showing an answer nothing
   * received. A caller with nothing to report may return nothing.
   */
  readonly onAnswer: (decision: string) => void | Promise<boolean>;
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

/**
 * The time the question was asked, or nothing when it cannot be read.
 *
 * An unparseable timestamp draws no time rather than "Invalid Date": §18.1's
 * rule for an event that predates timestamps is that no time is rendered, and
 * a broken one is the same fact arriving differently.
 */
function askedAtLabel(at: string): string | null {
  const parsed = Date.parse(at);
  return Number.isNaN(parsed) ? null : timeOfDay(parsed);
}

/**
 * The kernel's own sentence, with its inline code drawn as code.
 *
 * §18.3 asks for exactly that and no more: the wording is the kernel's and
 * the only thing this surface does to it is stop rendering its backticks as
 * punctuation. Parsed as one paragraph, because a question is one.
 */
function questionOf(message: string) {
  const first = parse(message).children[0];
  return first?.type === 'paragraph' ? renderInline(first.children) : message;
}

export function ApprovalCard({ interrupt, onAnswer, busy = false }: ApprovalCardProps) {
  /*
   * The answer this reader gave, held here from the moment they press.
   *
   * The card is cleared by the durable event, which arrives a whole lap of the
   * stream after the request returns — so answering used to grey the buttons,
   * ungrey them when the request resolved, and leave the card sitting there
   * unchanged until the log caught up. It read as a click that did not
   * register, and in the window between the two the answers were live again,
   * so it could be answered twice.
   *
   * Committing here is the same rule §18.1 already holds for a message being
   * typed: shown at once, replaced by the record, and identical either way
   * after a reload — because what is drawn from this never enters the log.
   */
  const [sent, setSent] = useState<string | undefined>(undefined);
  /*
   * A different question is a different answer: without this, a second
   * approval arriving in this same place inherits the first one's.
   *
   * Adjusted during render rather than in an effect. An effect would run after
   * the card had already been drawn once carrying the previous answer, and
   * setting state from one is the cascading render the lint rule is about.
   */
  const [asked, setAsked] = useState(interrupt.id);
  if (asked !== interrupt.id) {
    setAsked(interrupt.id);
    setSent(undefined);
  }
  const answers =
    interrupt.options === undefined || interrupt.options.length === 0
      ? PROTOCOL_ANSWERS
      : interrupt.options;

  return (
    <section className="approval" aria-label="Approval requested">
      {/*
       * The eyebrow — §18.3: approval, the kernel's own reason, and the time
       * it was asked. The time was required from the day the section was
       * written and never drawn; on a session picked up later it is the
       * difference between "the agent is waiting on me" and "the agent has
       * been waiting on me since this morning".
       *
       * Omitted rather than guessed where the log carried none, on §18.1's
       * rule for a message whose event predates timestamps.
       */}
      <p className="approval__eyebrow">
        approval · {interrupt.reason}
        {interrupt.askedAt === undefined ? null : (
          <span className="approval__asked"> · {askedAtLabel(interrupt.askedAt)}</span>
        )}
      </p>

      {/*
       * What the kernel wants, in the kernel's own words, as the largest type
       * on the surface — it is the sentence the decision turns on. Verbatim:
       * a kernel that says "Run `rm -rf build/`?" is asking about that
       * command, and rewording it into a category loses the one detail the
       * decision turns on.
       */}
      {/*
       * Run through the inline pass, because §18.3 says "A command inside it
       * takes inline code" and the service really does send them: a kernel
       * asking "Run `rm -rf build/`?" was showing its backticks as
       * punctuation, on the one sentence in the product where the exact
       * command is the whole decision.
       *
       * The inline pass only, never the block renderer: §18.3 fixes this line
       * at `--text-title` + 1, and `AgentContent` would impose its own
       * paragraph typography over it.
       */}
      <p className="approval__question">{questionOf(interrupt.message)}</p>

      {/*
       * The answers the kernel offered, rendered from what arrived. A kernel
       * offering three renders three; this component has no opinion about how
       * many there are or what they are called. Beside them, the fact that
       * makes it safe to take a moment: nothing else is waiting on this.
       */}
      <div className="approval__answers">
        {/*
         * Once answered, the one chosen is all that is drawn. The others are
         * withdrawn rather than disabled: they are not unavailable, they are
         * no longer the question, and a row of greyed alternatives beside the
         * answer invites somebody to press one again.
         */}
        {answers
          .filter((answer) => sent === undefined || answer.decision === sent)
          .map((answer) => (
            <Button
              key={answer.decision}
              size="md"
              variant={answer.affirmative === true ? 'primary' : 'ghost'}
              disabled={busy || sent !== undefined}
              onClick={() => {
                setSent(answer.decision);
                const outcome = onAnswer(answer.decision);
                // A refusal puts the question back rather than leaving the
                // card showing an answer the service never took.
                void Promise.resolve(outcome).then((taken) => {
                  if (taken === false) {
                    setSent(undefined);
                  }
                });
              }}
            >
              {answer.label ?? answer.decision}
            </Button>
          ))}
        <span className="approval__aside">
          {sent === undefined ? 'Your other sessions keep working.' : 'Sending your answer…'}
        </span>
      </div>
    </section>
  );
}
