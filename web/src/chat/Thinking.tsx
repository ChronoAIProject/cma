import { useEffect, useRef, useState } from 'react';
import './Thinking.css';

/**
 * What the agent is reasoning about, while it reasons — `Web_Style.md` §18.8.
 *
 * **It is liveness and it is never in the record.** It is removed when the
 * turn ends and a reload shows none of it. That is stated on the control
 * whether it is open or not, because a reader who saw reasoning and reloaded
 * to find it gone would otherwise report data loss.
 *
 * **Collapsed by default** to one pill key. The previous revision drew an
 * always-open band, which put the least durable content on the surface at the
 * same weight as the conversation. Opening it shows a height-capped band on
 * the raised ground, pinned to the newest.
 *
 * No colour, ever. §2 reserves the semantic inks for session and system
 * state, and reasoning is neither — it says nothing about how the session is
 * doing.
 */

interface ThinkingProps {
  readonly text: string;
}

export function Thinking({ text }: ThinkingProps) {
  const [open, setOpen] = useState(false);
  const bodyRef = useRef<HTMLDivElement>(null);
  /*
   * How long this reasoning has been arriving, counted from mount. Liveness
   * about liveness: the figure is not kept either, and a reload starts from
   * nothing because the reasoning it measured is gone.
   */
  const [seconds, setSeconds] = useState(0);

  useEffect(() => {
    const startedAt = Date.now();
    const timer = window.setInterval(() => {
      setSeconds(Math.floor((Date.now() - startedAt) / 1000));
    }, 1000);
    return () => {
      window.clearInterval(timer);
    };
  }, []);

  useEffect(() => {
    // Always pinned, unlike the message stream. Nobody scrolls back through
    // reasoning — it is going to be gone in a moment — so the newest is the
    // only part worth showing.
    const body = bodyRef.current;
    if (body !== null && open) {
      body.scrollTop = body.scrollHeight;
    }
  }, [text, open]);

  return (
    <section className="thinking" aria-label="What the agent is working through">
      {/*
       * The words that make it safe to lose stay on the key whether it is
       * open or not — §18.8.
       */}
      <button
        type="button"
        className="thinking__key"
        aria-expanded={open}
        onClick={() => {
          setOpen((wasOpen) => !wasOpen);
        }}
      >
        Thought for {seconds}s · not kept after this turn
      </button>

      {/*
       * Not a live region. It changes many times a second, and announcing
       * each change would make the surface unusable with a screen reader; the
       * message that follows is what is worth hearing, and it is announced.
       */}
      {open ? (
        <div className="thinking__body" ref={bodyRef}>
          {text}
        </div>
      ) : null}
    </section>
  );
}
