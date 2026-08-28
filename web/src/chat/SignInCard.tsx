import type { SignIn } from '@chronoai/cma-sdk';
import { CopyKey } from './CopyKey';
import './SignInCard.css';

/**
 * The agent is waiting for its owner to sign in — `Web_Style.md` §18.
 *
 * Drawn only on a session spending its owner's own account with the model
 * vendor. It is the one thing on this plane that has to be finished somewhere
 * else, on another device, so the card's whole job is to be actionable from
 * across the room: the code large enough to read and copy, the address plain
 * enough to type.
 *
 * It sits in the stream where the turn stopped, on the same reasoning §18.3
 * gives for an approval card and explicitly **not as a modal** — a session can
 * wait on this while its owner reads something else, and a modal would make
 * every other session unreachable until they finished.
 *
 * **Nothing here is a credential.** The address and the code are worthless
 * without the person's own account, and what the sign-in produces is written
 * by the agent inside its own sandbox and never reaches this service. That is
 * what §18.7's "never renders a credential" permits this card to exist at all.
 */

interface SignInCardProps {
  readonly signIn: SignIn;
}

export function SignInCard({ signIn }: SignInCardProps) {
  return (
    <section className="signin" aria-label="Sign in to continue">
      <p className="signin__eyebrow">sign in · your own account</p>

      {/*
       * The kernel's own sentence where it gave one, verbatim, on the rule
       * §18.2 and §18.10 both hold: it knows what it is asking for and this
       * client does not. A fallback only when it said nothing at all.
       */}
      <p className="signin__what">
        {signIn.message ??
          'This session runs on your own account. Open the page below and sign in to continue.'}
      </p>

      {signIn.code === undefined ? null : (
        <div className="signin__coderow">
          {/*
           * The code is the thing being carried to another device, so it is
           * the largest type on the card and spaced to be read aloud or typed
           * without losing your place.
           */}
          <code className="signin__code">{signIn.code}</code>
          <CopyKey text={signIn.code} label="Copy code" />
        </div>
      )}

      <div className="signin__go">
        <a className="signin__link" href={signIn.url} target="_blank" rel="noopener noreferrer">
          {signIn.url}
        </a>
        <CopyKey text={signIn.url} label="Copy link" />
      </div>

      {/*
       * Two facts a person cannot discover from here and will otherwise
       * discover by failing.
       *
       * The precondition is real: device-code sign-in is off by default on a
       * consumer account and has to be enabled by its owner, and on a managed
       * workspace an administrator controls it. A subscription alone is not
       * enough, and §9 asks for that to be said up front rather than found
       * halfway through.
       *
       * The expiry is the kernel's, not ours — codes of this kind are
       * short-lived by construction, and somebody who leaves this open
       * overnight should know the code went stale rather than think the
       * feature is broken.
       */}
      <p className="signin__note">
        The code expires shortly. If the page refuses it, device sign-in may need turning on in your
        account&rsquo;s security settings first — on a managed workspace, by an administrator.
      </p>
    </section>
  );
}
