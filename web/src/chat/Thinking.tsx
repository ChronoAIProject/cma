import { useEffect, useRef, useState } from 'react';
import './Thinking.css';
import { IconThinkOutline14 } from '../ui/icons';

/**
 * What the agent is reasoning about, while it reasons — `Web_Style.md` §18.8.
 *
 * **It is liveness and it is never in the record.** It is removed when the
 * turn ends and a reload shows none of it. That is stated on the control
 * whether it is open or not, because a reader who saw reasoning and reloaded
 * to find it gone would otherwise report data loss.
 *
 * **Collapsed by default** to one row. The previous revision drew an
 * always-open band, which put the least durable content on the surface at the
 * same weight as the conversation. Opening it shows a height-capped band
 * pinned to the newest.
 *
 * The row carries the newest line while it is collapsed. That is the whole
 * reason to look at a shut row: "it is doing something" is what a spinner
 * says, and this says what.
 *
 * There is deliberately no settled summary. The design this follows shows the
 * first line once reasoning has stopped — but reasoning here never stops on
 * screen: §18.8 makes it liveness, so the run ending removes it entirely and
 * a row showing a finished summary is a row that cannot exist.
 *
 * No colour, ever. §2 reserves the semantic inks for session and system
 * state, and reasoning is neither — it says nothing about how the session is
 * doing.
 */

interface ThinkingProps {
  readonly text: string;
}

/** The newest line, which is what the agent is working through right now. */
export function summaryOf(text: string): string {
  const lines = text
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
  return lines[lines.length - 1] ?? '';
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
        <span className="thinking__glyph" aria-hidden="true">
          <IconThinkOutline14 />
        </span>
        <span className="thinking__title">Thought for {seconds}s</span>
        <span className="thinking__dot" aria-hidden="true" />
        {/*
          The summary is the row's reason to exist while it is shut. It never
          wraps: a row that grows with the agent's longest sentence moves the
          conversation under it several times a second.
        */}
        <span className="thinking__summary">{summaryOf(text)}</span>
        <span className="sr-only">, not kept after this turn</span>
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
