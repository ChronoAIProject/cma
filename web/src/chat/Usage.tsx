import type { Usage as UsageState } from './conversation';
import './Usage.css';

/**
 * What the session has cost — `Web_Style.md` §18.5.
 *
 * **Absence is a fact, not a zero.** `usage` is a granted capability, and
 * `capabilities.rs` withholds it from the terminal plane entirely: "steering
 * and usage belong to the conversation plane — they are expressed in turns and
 * tokens, and a screen has neither." A kernel may not report it either.
 *
 * So a session that cannot report usage says so, and **no reading of the
 * interface suggests zero**. This is the same rule §14 already holds for the
 * operations page, which withholds a running total while any provider is
 * unobservable rather than showing a total that is wrong — and it is the same
 * reason: a reading that suggests "nothing has been spent" when the truth is
 * "we cannot tell" is a lie the interface told, and a session with no usage
 * grant has often spent a great deal.
 *
 * Nothing here is estimated. A number this interface computed would be a
 * number nobody can reconcile with a bill.
 */

interface UsageProps {
  /** Whether the session was granted `usage` at all. */
  readonly granted: boolean;
  /** What it has reported, when it has reported anything. */
  readonly usage?: UsageState;
}

export function Usage({ granted, usage }: UsageProps) {
  if (!granted) {
    return (
      <span className="usage usage--absent">
        <span className="band__label">Usage</span>
        <span className="band__value">not reported</span>
      </span>
    );
  }

  /*
   * Granted, and nothing has arrived yet. Still not a zero: the session has
   * simply not said, and it will. Drawn as a waiting value rather than as a
   * total, for the same reason as above.
   */
  if (usage?.tokens === undefined && usage?.cost === undefined) {
    return (
      <span className="usage">
        <span className="band__label">Usage</span>
        <span className="band__value">waiting</span>
      </span>
    );
  }

  return (
    <span className="usage">
      <span className="band__label">Usage</span>
      {usage.tokens === undefined ? null : (
        <span className="band__value usage__tokens">
          {usage.tokens.toLocaleString('en')} tokens
        </span>
      )}
      {usage.cost === undefined ? null : <span className="band__value">{usage.cost}</span>}
    </span>
  );
}
