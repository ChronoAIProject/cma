import { useEffect, useRef } from 'react';
import { BouncingText } from '../sessions/BouncingText';
import './Thinking.css';

/**
 * What the agent is reasoning about, while it reasons — `Web_Style.md` §18.8.
 *
 * **It is liveness and it is never in the record.** It is removed when the
 * turn ends and a reload shows none of it. That is stated on the surface,
 * because a reader who saw reasoning and reloaded to find it gone would
 * otherwise report data loss.
 *
 * No colour, ever. §2 reserves the semantic inks for session and system state,
 * and reasoning is neither — it says nothing about how the session is doing.
 *
 * One band, height-capped, scrolled to the newest. Reasoning is unbounded and
 * arrives fast, and a band that grew without limit would push the conversation
 * off the screen while saying less than any line of it.
 */

interface ThinkingProps {
  readonly text: string;
}

export function Thinking({ text }: ThinkingProps) {
  const bodyRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    // Always pinned, unlike the message stream. Nobody scrolls back through
    // reasoning — it is going to be gone in a moment — so the newest is the
    // only part worth showing.
    const body = bodyRef.current;
    if (body !== null) {
      body.scrollTop = body.scrollHeight;
    }
  }, [text]);

  return (
    <section className="thinking" aria-label="What the agent is working through">
      <span className="thinking__label">
        <BouncingText text="Thinking" />
      </span>
      {/*
       * Not a live region. It changes many times a second, and announcing each
       * change would make the surface unusable with a screen reader; the
       * message that follows is what is worth hearing, and it is announced.
       */}
      <div className="thinking__body" ref={bodyRef}>
        {text}
      </div>
      <p className="thinking__note">Not kept. This is gone when the turn ends.</p>
    </section>
  );
}
