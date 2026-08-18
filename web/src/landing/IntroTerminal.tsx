import { useEffect, useRef, useState } from 'react';
import type { KeyboardEvent as ReactKeyboardEvent } from 'react';
import { ACCESSIBLE_TRANSCRIPT, INTRO_LINES, TIMING, lineText, OPENING_LINE } from './script';
import { useIntroAnimation } from './useIntroAnimation';
import { MAX_TURN_LENGTH, capturePendingTurn } from '../auth/pendingTurn';
import { useAuth } from '../auth/context';
import './IntroTerminal.css';

/**
 * The signed-out surface — docs/Web_Style.md §15.
 *
 * "A statement, and a terminal that already works." A statement column beside
 * a live terminal panel, inside the same page frame as every other page: a
 * visitor and an owner are looking at one product.
 *
 * Three structural decisions drive everything here.
 *
 * The input is a genuine <input>, mounted and **focused from the first frame**.
 * Time-to-interactive is therefore zero and the animation is decoration over an
 * already-usable field — a visitor who types at 300ms keeps every character.
 * Deferring focus until the animation finished would silently eat their
 * keystrokes, which is the worst failure available to this design.
 *
 * The visible text IS input.value. No mirror element renders it, so the whole
 * class of mirror-desync defects (IME composition, mobile autocorrect,
 * selection, RTL) cannot occur.
 *
 * The sign-in affordance appears only once the visitor has typed a character.
 * §15: do not nag an empty prompt.
 */

export function IntroTerminal() {
  const { signIn } = useAuth();
  const { frame, complete, skip, reducedMotion } = useIntroAnimation();

  const inputRef = useRef<HTMLInputElement>(null);
  /*
   * §10 and §15: focus on load rests on the prompt, so a keyboard visitor can
   * type immediately without hunting. Done on mount rather than when the
   * animation finishes, which is what makes time-to-interactive zero rather
   * than 3.2 seconds.
   */
  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const [value, setValue] = useState('');
  const [skipOffered, setSkipOffered] = useState(false);
  const [showHint, setShowHint] = useState(reducedMotion);
  const [status, setStatus] = useState('');
  const [emptyEnters, setEmptyEnters] = useState(0);
  const [capturing, setCapturing] = useState(false);
  // The opening line types itself once the boot finishes, so the prompt shows
  // what to do rather than an empty field and a blinking caret. It stays
  // editable: it is a suggestion, not a script.
  const [suggested, setSuggested] = useState(false);
  // Whether the visitor has typed anything themselves. The suggestion must
  // never overwrite their words: someone who starts typing during the
  // introduction has said what they want, and finishing our sentence over the
  // top of theirs is the worst thing this could do.
  const [touched, setTouched] = useState(false);

  // The skip control appears only after the boot block, the first point at
  // which "how long is this?" can occur. Offering it at 0ms would frame the
  // animation as an obstacle before the visitor decided it was one.
  useEffect(() => {
    if (reducedMotion || complete) {
      return;
    }
    const timer = window.setTimeout(() => {
      setSkipOffered(true);
    }, TIMING.skipRevealAt);
    return () => {
      window.clearTimeout(timer);
    };
  }, [reducedMotion, complete]);

  useEffect(() => {
    if (!complete || suggested || touched) {
      return;
    }
    // Reduced motion arrives in one step rather than by a separate path, so
    // both cases share the same guard against overwriting the visitor.
    const step = reducedMotion ? OPENING_LINE.length : 1;
    let index = 0;
    const timer = window.setInterval(() => {
      index += step;
      setValue(OPENING_LINE.slice(0, index));
      if (index >= OPENING_LINE.length) {
        window.clearInterval(timer);
        setSuggested(true);
      }
    }, TIMING.typeInterval);
    return () => {
      window.clearInterval(timer);
    };
  }, [complete, reducedMotion, suggested, touched]);

  // Derived, not stored: once the animation finishes there is nothing to skip,
  // and leaving dead chrome on the page is worse than removing it.
  const showSkip = skipOffered && !complete && !reducedMotion;

  // Escape skips from anywhere, including while the input has focus, which is
  // where it will be. Bound on the document because the control advertises
  // "esc" and a key that does nothing is worse than no affordance at all.
  useEffect(() => {
    if (complete) {
      return;
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        skip();
      }
    };
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [complete, skip]);

  // The Enter hint waits for settled intent: enough characters to be more than
  // exploratory, then a pause in typing. Revealing it mid-keystroke would put
  // motion in peripheral vision exactly while someone is composing. Once shown
  // it never hides again — a hint that flickers per keystroke is the real nag.
  useEffect(() => {
    if (showHint || value.trim().length < TIMING.hintMinChars) {
      return;
    }
    const timer = window.setTimeout(() => {
      setShowHint(true);
    }, TIMING.hintIdleMs);
    return () => {
      window.clearTimeout(timer);
    };
  }, [value, showHint]);

  /**
   * Hold the visitor's words and take them to sign in.
   *
   * Shared by Enter and by the statement's opening action, so the two cannot
   * come to mean different things — and so the text survives the round trip
   * either way and becomes the first turn of their first session.
   */
  function submit() {
    const text = value.trim();
    if (text.length === 0 || capturing) {
      return;
    }
    capturePendingTurn(text);
    setCapturing(true);
    // Announced because a redirect that nothing announces is genuinely
    // disorienting for a screen reader user.
    setStatus('Holding your prompt. Redirecting to sign in.');
    void signIn();
  }

  function handleKeyDown(event: ReactKeyboardEvent<HTMLInputElement>) {
    // Any keystroke completes the animation. Deliberately not preventDefault:
    // the character falls through into the input, so the impatient visitor's
    // instinct to just start typing is also the primary skip affordance.
    if (!complete) {
      skip();
    }

    if (event.key !== 'Enter' || event.shiftKey || capturing) {
      return;
    }
    event.preventDefault();

    const text = value.trim();

    if (text.length === 0) {
      if (emptyEnters === 0) {
        setEmptyEnters(1);
        setStatus('Type something first, or press Enter again to sign in.');
        return;
      }
      setCapturing(true);
      setStatus('Redirecting to sign in.');
      void signIn();
      return;
    }

    submit();
  }

  const visible = INTRO_LINES.slice(0, frame.visibleLines);

  return (
    <section className="intro" aria-labelledby="intro-heading">
      {/*
        The static equivalent of the animation, present from first paint and
        deliberately NOT a live region. Character-by-character mutation inside a
        live region produces a torrent of partial-word announcements that no
        politeness setting fixes; the animation is therefore removed from the
        accessibility tree entirely and this is read instead.
      */}
      <p className="sr-only">{ACCESSIBLE_TRANSCRIPT.join(' ')}</p>

      {/* The statement — §15. */}
      <div className="intro__statement">
        <h1 id="intro-heading" className="intro__headline">
          Nothing to install.
        </h1>
        <p className="intro__lede">
          Managed coding agents on ChronoAI infrastructure. Say what you want built, and sign in to
          run it.
        </p>
        {/*
         * The sign-in affordance, offered only once the visitor has typed a
         * character (§15). An empty prompt is not nagged: somebody who has
         * said nothing has not yet asked for anything to happen.
         */}
        {value.trim().length > 0 ? (
          <button
            type="button"
            className="button button--primary button--xl"
            disabled={capturing}
            onClick={() => {
              submit();
            }}
          >
            Sign in and run it
          </button>
        ) : null}
      </div>

      <div
        className="intro__frame"
        onClick={() => {
          // §15: any keypress or click completes the animation. A click that
          // only moved focus left the one gesture a visitor reaches for when
          // they have decided the introduction is an obstacle doing nothing.
          if (!complete) {
            skip();
          }
          inputRef.current?.focus();
        }}
      >
        <pre className="intro__out" aria-hidden="true">
          {visible.map((line, lineIndex) => {
            const typed = line.typed === true;
            const isLast = lineIndex === visible.length - 1;
            // Only the final visible line is mid-reveal; earlier ones are whole.
            const budget = typed && isLast && !complete ? frame.charsOnLastLine : Infinity;

            let spent = 0;
            return (
              <span className="intro__line" key={lineIndex}>
                {line.segments.map((segment, segmentIndex) => {
                  const remaining = Math.max(0, budget - spent);
                  const shown = segment.text.slice(0, remaining);
                  spent += segment.text.length;
                  return (
                    <span className={`tone-${segment.tone}`} key={segmentIndex}>
                      {shown}
                    </span>
                  );
                })}
                {typed && isLast && !complete ? <b className="intro__caret" /> : null}
                {'\n'}
              </span>
            );
          })}
        </pre>

        <div className="intro__row" data-live={complete ? 'true' : 'false'}>
          <span className="intro__ps1" aria-hidden="true">
            <span className="tone-accent">guest</span>
            <span className="tone-accent">@chrono-bot</span>
            <span className="tone-dim">:~$</span>
          </span>
          <label className="sr-only" htmlFor="intro-prompt">
            What would you like ChronoBot to build? Press Enter to sign in and run it.
          </label>
          <input
            id="intro-prompt"
            ref={inputRef}
            className="intro__input"
            type="text"
            value={value}
            autoComplete="off"
            spellCheck={false}
            maxLength={MAX_TURN_LENGTH}
            disabled={capturing}
            aria-describedby="intro-contract"
            onChange={(event) => {
              setValue(event.target.value);
              setEmptyEnters(0);
              // From here the words are theirs; the suggestion stops.
              setTouched(true);
            }}
            onKeyDown={handleKeyDown}
          />
          <span id="intro-contract" className="sr-only">
            Your text is saved and submitted after you sign in.
          </span>
        </div>

        {suggested || showHint ? (
          <p className="intro__hint">
            {/*
             * Blinks so it reads as the terminal waiting on the visitor rather
             * than as a static label, and is announced once as ordinary text.
             */}
            <span className="intro__blink" aria-hidden="true">
              ↵
            </span>{' '}
            Enter to continue
          </p>
        ) : null}

        <p className="intro__status sr-only" aria-live="polite" aria-atomic="true">
          {status}
        </p>
      </div>

      {showSkip ? (
        <button type="button" className="intro__skip" onClick={skip}>
          esc — skip
        </button>
      ) : null}
    </section>
  );
}

/** Exported for tests that assert the printed script matches the copy. */
export function renderedText(visibleLines: number): string {
  return INTRO_LINES.slice(0, visibleLines).map(lineText).join('\n');
}
