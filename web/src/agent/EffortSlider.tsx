import { useRef, type KeyboardEvent, type PointerEvent } from 'react';

export interface SliderStop<Value extends string> {
  readonly value: Value;
  readonly label: string;
}

/**
 * A discrete slider over named stops — `docs/Web_Style.md` §13.
 *
 * Reasoning effort is an ordered scale, so it reads better as a position than
 * as a list. The control is one focusable `slider` that announces the stop's
 * name rather than its index, moves by one stop with the arrow keys and Page
 * Up/Down, and jumps to either end with Home and End. A press or drag on the
 * track snaps to the nearest stop.
 */
export function EffortSlider<Value extends string>({
  label,
  value,
  stops,
  disabled = false,
  onChange,
}: {
  readonly label: string;
  readonly value: Value;
  readonly stops: readonly SliderStop<Value>[];
  readonly disabled?: boolean;
  readonly onChange: (value: Value) => void;
}) {
  const track = useRef<HTMLDivElement>(null);
  const index = Math.max(
    0,
    stops.findIndex((stop) => stop.value === value),
  );
  const last = stops.length - 1;
  const percent = last <= 0 ? 0 : (index / last) * 100;

  const choose = (next: number) => {
    const clamped = Math.min(last, Math.max(0, next));
    const stop = stops[clamped];
    if (stop !== undefined && clamped !== index) onChange(stop.value);
  };

  const fromPointer = (event: PointerEvent<HTMLDivElement>) => {
    const box = track.current?.getBoundingClientRect();
    if (box === undefined || box.width <= 0 || last <= 0) return;
    const ratio = (event.clientX - box.left) / box.width;
    choose(Math.round(ratio * last));
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (disabled) return;
    const moves: Record<string, number> = {
      ArrowRight: index + 1,
      ArrowUp: index + 1,
      PageUp: index + 1,
      ArrowLeft: index - 1,
      ArrowDown: index - 1,
      PageDown: index - 1,
      Home: 0,
      End: last,
    };
    const next = moves[event.key];
    if (next === undefined) return;
    event.preventDefault();
    choose(next);
  };

  return (
    <div
      ref={track}
      className="effort-slider"
      role="slider"
      tabIndex={disabled ? -1 : 0}
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={last}
      aria-valuenow={index}
      aria-valuetext={stops[index]?.label}
      aria-disabled={disabled || undefined}
      aria-orientation="horizontal"
      data-autofocus=""
      onKeyDown={onKeyDown}
      onPointerDown={(event) => {
        if (disabled || event.button !== 0) return;
        event.currentTarget.setPointerCapture?.(event.pointerId);
        event.currentTarget.focus({ preventScroll: true });
        fromPointer(event);
      }}
      onPointerMove={(event) => {
        if (disabled || !event.currentTarget.hasPointerCapture?.(event.pointerId)) return;
        fromPointer(event);
      }}
    >
      <span className="effort-slider__rail" aria-hidden="true">
        <span className="effort-slider__fill" style={{ width: `${String(percent)}%` }} />
        {stops.map((stop, at) => (
          <span
            key={stop.value}
            className="effort-slider__stop"
            data-reached={at <= index ? '' : undefined}
            style={{ left: `${String(last <= 0 ? 0 : (at / last) * 100)}%` }}
          />
        ))}
        <span className="effort-slider__thumb" style={{ left: `${String(percent)}%` }} />
      </span>
    </div>
  );
}
