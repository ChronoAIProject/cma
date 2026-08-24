import type { Usage as UsageState } from '@chronoai/cma-sdk';
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

/**
 * A token count, at the size somebody reads it at.
 *
 * Grouped digits up to a thousand and abbreviated past it. A six-figure count
 * printed in full is a number nobody reads — what is being answered is "how
 * much of this have I spent", and 128K answers it where 128,431 does not.
 *
 * One decimal under a hundred, because the difference between 1.2K and 1.9K
 * is worth seeing and the difference between 128K and 129K is not.
 */
export function formatTokens(tokens: number): string {
  if (tokens < 1000) {
    return tokens.toLocaleString('en');
  }
  if (tokens < 100_000) {
    return `${(tokens / 1000).toFixed(1)}K`;
  }
  if (tokens < 1_000_000) {
    return `${Math.round(tokens / 1000).toLocaleString('en')}K`;
  }
  return `${(tokens / 1_000_000).toFixed(1)}M`;
}

export function Usage({ granted, usage }: UsageProps) {
  if (!granted) {
    return <span className="usage">Usage not reported for this session</span>;
  }

  /*
   * Granted, and nothing has arrived yet. Still not a zero: the session has
   * simply not said, and it will. Drawn as a waiting value rather than as a
   * total, for the same reason as above.
   */
  if (usage?.tokens === undefined && usage?.cost === undefined) {
    return <span className="usage">Usage · waiting</span>;
  }

  return (
    <span className="usage">
      {usage.tokens === undefined ? null : (
        <span className="usage__tokens">{formatTokens(usage.tokens)} tokens</span>
      )}
      {usage.cost === undefined ? null : <span>{usage.cost}</span>}
    </span>
  );
}
