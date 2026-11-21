import { useEffect, useId, useRef, useState } from 'react';
import type { AgentUsage } from '../api/types';
import { contextFraction, usageDefinitions, usageSummary } from './usageDescription';
import './ContextRing.css';

/** The ring's radius in its 20px box, leaving room for the stroke. */
const RADIUS = 8;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;

/**
 * Context and token usage as one ring in the composer — the shared style contracts
 *
 * The figures used to take a whole text row under the input. They are read
 * occasionally, so the ring carries the one thing worth seeing at a glance —
 * how full the context window is — and the tooltip holds the rest.
 *
 * The tooltip stays in the document and describes the button, so a screen
 * reader hears the figures on focus without opening anything; hover, focus or
 * a press only decides whether it is drawn.
 *
 * Absence is never drawn as zero: without both context figures the ring is
 * only its track, and every unreported figure says so in words.
 */
export function ContextRing({ usage }: { readonly usage?: AgentUsage }) {
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const [pressed, setPressed] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const tooltipId = useId();
  const root = useRef<HTMLSpanElement>(null);
  const fraction = contextFraction(usage);
  const { context, total, split, turn } = usageSummary(usage);
  const open = (hovered || focused || pressed) && !dismissed;
  // A press does not focus a button in every browser, so a tooltip opened by
  // touch is closed by the next press anywhere else rather than by blur alone.
  useEffect(() => {
    if (!pressed) return;
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && root.current?.contains(event.target) === true) return;
      setPressed(false);
    };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, [pressed]);
  return (
    <span
      ref={root}
      className="context-ring"
      onPointerEnter={() => {
        setDismissed(false);
        setHovered(true);
      }}
      onPointerLeave={() => setHovered(false)}
    >
      <button
        type="button"
        className="context-ring__button"
        aria-label="Context usage"
        aria-describedby={tooltipId}
        aria-expanded={open}
        onFocus={() => {
          setDismissed(false);
          setFocused(true);
        }}
        onBlur={() => {
          setFocused(false);
          setPressed(false);
        }}
        onClick={() => {
          setDismissed(false);
          setPressed((was) => !was);
        }}
        onKeyDown={(event) => {
          // Escape hides the tooltip where it is; focus stays on the ring.
          if (event.key === 'Escape' && open) {
            event.stopPropagation();
            setDismissed(true);
            setPressed(false);
          }
        }}
      >
        <svg viewBox="0 0 20 20" width="20" height="20" aria-hidden="true" focusable="false">
          <circle className="context-ring__track" cx="10" cy="10" r={RADIUS} />
          {fraction === undefined || fraction === 0 ? null : (
            <circle
              className="context-ring__arc"
              cx="10"
              cy="10"
              r={RADIUS}
              strokeDasharray={`${String(CIRCUMFERENCE * fraction)} ${String(CIRCUMFERENCE)}`}
              transform="rotate(-90 10 10)"
            />
          )}
        </svg>
      </button>
      <span id={tooltipId} role="tooltip" className="context-ring__tooltip" hidden={!open}>
        <span className="context-ring__row">
          <span className="context-ring__term">Context</span>{' '}
          <span className="context-ring__value">{context}</span>{' '}
          <span className="context-ring__definition">{usageDefinitions.context}</span>
        </span>{' '}
        <span className="context-ring__row">
          <span className="context-ring__term">Total tokens</span>{' '}
          <span className="context-ring__value">
            {total}
            {split === undefined ? null : (
              <>
                {' '}
                <span className="context-ring__detail">{split}</span>
              </>
            )}
          </span>{' '}
          <span className="context-ring__definition">{usageDefinitions.total}</span>
        </span>{' '}
        {turn === undefined ? null : (
          <>
            <span className="context-ring__row">
              <span className="context-ring__term">This turn</span>{' '}
              <span className="context-ring__value">{turn}</span>{' '}
              <span className="context-ring__definition">{usageDefinitions.turn}</span>
            </span>{' '}
          </>
        )}
        <span>{usageDefinitions.billing}</span>
      </span>
    </span>
  );
}
