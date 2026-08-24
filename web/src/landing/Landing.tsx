import { useEffect, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import { MAX_TURN_LENGTH, capturePendingTurn } from '../auth/pendingTurn';
import { useAuth } from '../auth/context';
import { Button } from '../ui/Button';
import { LandingCharacter } from './LandingCharacter';
import './Landing.css';

/**
 * The signed-out surface — `docs/Web_Style.md` §20.
 *
 * The live character, at a size where its expression is legible, and one thing
 * to do.
 *
 * This replaced a simulated terminal that typed a scripted boot log beside a
 * statement column. It was demonstrating the product by pretending to be it,
 * and the pretence was the problem: everything on it was fake except the input,
 * so the one real affordance was the least conspicuous thing on the page.
 *
 * Two things are kept from it deliberately.
 *
 * The prompt is a genuine <input>, mounted and focused from the first frame,
 * so time-to-interactive is zero and a visitor who types immediately keeps
 * every character. Nothing here defers on an animation.
 *
 * What they type is held across the sign-in redirect and run as the first turn
 * of their first session (`capturePendingTurn`). That is the whole reason a
 * signed-out visitor is given a field at all rather than a bare button — the
 * question "what would you like built" is answerable before an account exists,
 * and making somebody retype their answer after signing in is the kind of
 * small insult a product only gets to make once.
 */

export function Landing() {
  const { signIn } = useAuth();
  const inputRef = useRef<HTMLInputElement>(null);
  const [value, setValue] = useState('');
  const [leaving, setLeaving] = useState(false);
  const [status, setStatus] = useState('');

  /*
   * §10 and §15: focus on load rests on the prompt, so a keyboard visitor can
   * type without hunting for the one control on the page.
   */
  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  function submit(event: FormEvent) {
    event.preventDefault();
    if (leaving) {
      return;
    }

    const text = value.trim();
    setLeaving(true);
    if (text.length > 0) {
      capturePendingTurn(text);
      // Announced because a redirect that nothing announces is genuinely
      // disorienting for a screen reader user.
      setStatus('Holding your prompt. Redirecting to sign in.');
    } else {
      setStatus('Redirecting to sign in.');
    }
    void signIn();
  }

  return (
    <main className="landing">
      <div className="landing__stack">
        <LandingCharacter />

        <h1 className="landing__word">CMA</h1>
        <p className="landing__expand">ChronoAI Managed Agent</p>

        <form className="landing__ask" onSubmit={submit}>
          <label className="sr-only" htmlFor="landing-prompt">
            What would you like CMA to build? Press Enter to sign in and run it.
          </label>
          <input
            id="landing-prompt"
            ref={inputRef}
            className="landing__prompt"
            type="text"
            value={value}
            placeholder="What would you like CMA to build?"
            autoComplete="off"
            spellCheck={false}
            maxLength={MAX_TURN_LENGTH}
            disabled={leaving}
            onChange={(event) => {
              setValue(event.target.value);
            }}
          />
          <Button type="submit" variant="primary" size="md" disabled={leaving}>
            {value.trim().length > 0 ? 'Sign in and run it' : 'Sign in'}
          </Button>
        </form>

        <p className="landing__note">
          Coding agents on ChronoAI infrastructure. Nothing to install.
        </p>
      </div>

      <p className="sr-only" role="status">
        {status}
      </p>
    </main>
  );
}
