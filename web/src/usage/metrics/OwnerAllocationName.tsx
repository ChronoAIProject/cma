import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { placeMenu } from '../../ui/menuPlacement';

export function ownerAllocationText(row: { label: string; disambiguator?: string }) {
  return row.disambiguator ? `${row.label} · ${row.disambiguator}` : row.label;
}

/** Keep full identity text outside the virtual scroll viewport's clipping box. */
export function OwnerAllocationName({
  label,
  disambiguator,
}: {
  readonly label: string;
  readonly disambiguator?: string;
}) {
  const id = useId();
  const anchor = useRef<HTMLSpanElement>(null);
  const panel = useRef<HTMLSpanElement>(null);
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const visible = (hovered || focused) && !dismissed;
  const text = ownerAllocationText({ label, disambiguator });
  const enter = () => {
    clearTimeout(timer.current);
    setHovered(true);
    setDismissed(false);
  };
  const leave = () => {
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setHovered(false), 150);
  };
  useEffect(() => () => clearTimeout(timer.current), []);
  useLayoutEffect(() => {
    if (!visible) return;
    const place = () => {
      const element = panel.current;
      const rect = anchor.current?.getBoundingClientRect();
      if (!element || !rect) return;
      const position = placeMenu(
        rect,
        { width: element.offsetWidth, height: element.offsetHeight },
        { width: innerWidth, height: innerHeight },
        'top',
        'start',
      );
      element.style.left = `${position.left}px`;
      element.style.top = `${position.top}px`;
      element.style.visibility = 'visible';
    };
    place();
    const observer = new ResizeObserver(place);
    if (anchor.current) observer.observe(anchor.current);
    if (panel.current) observer.observe(panel.current);
    window.addEventListener('scroll', place, true);
    window.addEventListener('resize', place);
    return () => {
      observer.disconnect();
      window.removeEventListener('scroll', place, true);
      window.removeEventListener('resize', place);
    };
  }, [visible, text]);
  return (
    <span
      ref={anchor}
      className="usage-owner-allocation__name"
      tabIndex={0}
      title={text}
      aria-describedby={visible ? id : undefined}
      onMouseEnter={enter}
      onMouseLeave={leave}
      onFocus={() => {
        setFocused(true);
        setDismissed(false);
      }}
      onBlur={() => setFocused(false)}
      onKeyDown={(event) => {
        if (event.key === 'Escape') setDismissed(true);
      }}
    >
      <span>{label}</span>
      {disambiguator && <small>{disambiguator}</small>}
      {visible &&
        createPortal(
          <span
            ref={panel}
            id={id}
            role="tooltip"
            className="usage-owner-allocation__full"
            style={{ visibility: 'hidden', top: 0, left: 0 }}
            onMouseEnter={enter}
            onMouseLeave={leave}
          >
            {text}
          </span>,
          document.body,
        )}
    </span>
  );
}
