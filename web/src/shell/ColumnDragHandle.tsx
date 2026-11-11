import { useCallback, useEffect, useRef, useState } from 'react';
import { clampWidth } from './columns';

interface DragHandleProps {
  readonly side: 'sidebar' | 'details';
  /** Where the border it straddles currently is. */
  readonly at: number;
  readonly label: string;
  readonly value: number;
  readonly min: number;
  readonly max: number;
  readonly onResize: (width: number) => void;
  readonly onDrag: (dx: number) => void;
  readonly onDraggingChange: (dragging: boolean) => void;
}

export function DragHandle({
  side,
  at,
  label,
  value,
  min,
  max,
  onResize,
  onDrag,
  onDraggingChange,
}: DragHandleProps) {
  const [dragging, setDragging] = useState(false);
  const origin = useRef(0);
  const latest = useRef(0);
  const frame = useRef<number | null>(null);

  useEffect(
    () => () => {
      if (frame.current !== null) cancelAnimationFrame(frame.current);
    },
    [],
  );

  const report = useCallback(() => {
    frame.current = null;
    onDrag(latest.current - origin.current);
  }, [onDrag]);

  return (
    <div
      className="frame__handle"
      style={{ left: at }}
      data-side={side}
      data-dragging={dragging ? 'true' : undefined}
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      tabIndex={value >= min ? 0 : -1}
      aria-valuenow={value}
      aria-valuemin={min}
      aria-valuemax={max}
      aria-valuetext={`${Math.round(value)} pixels`}
      onKeyDown={(event) => {
        if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
        event.preventDefault();
        const step = event.shiftKey ? 48 : 16;
        const direction = (event.key === 'ArrowRight' ? 1 : -1) * (side === 'details' ? -1 : 1);
        onResize(
          event.key === 'Home'
            ? min
            : event.key === 'End'
              ? max
              : clampWidth(value + direction * step, min, max),
        );
      }}
      onLostPointerCapture={() => {
        if (frame.current !== null) cancelAnimationFrame(frame.current);
        frame.current = null;
        setDragging(false);
        onDraggingChange(false);
      }}
      onPointerDown={(event) => {
        event.preventDefault();
        /*
         * Captured, so the drag survives the pointer leaving the 8px strip.
         * Without it a fast drag stops the moment the pointer outruns the
         * column, which is exactly when somebody is dragging fast.
         */
        event.currentTarget.setPointerCapture(event.pointerId);
        origin.current = event.clientX;
        latest.current = event.clientX;
        setDragging(true);
        onDraggingChange(true);
      }}
      onPointerMove={(event) => {
        if (!event.currentTarget.hasPointerCapture(event.pointerId)) {
          return;
        }
        latest.current = event.clientX;
        // One report per frame. Pointer events arrive faster than layout.
        frame.current ??= requestAnimationFrame(report);
      }}
      onPointerUp={(event) => {
        if (!event.currentTarget.hasPointerCapture(event.pointerId)) {
          return;
        }
        event.currentTarget.releasePointerCapture(event.pointerId);
        if (frame.current !== null) {
          cancelAnimationFrame(frame.current);
          frame.current = null;
        }
        // The last position, which the throttle may not have reported yet.
        onDrag(latest.current - origin.current);
        setDragging(false);
        onDraggingChange(false);
      }}
    />
  );
}
