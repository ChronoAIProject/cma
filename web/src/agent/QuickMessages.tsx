import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import type { TriggerQuickMessage } from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import { placeMenu } from '../ui/menuPlacement';

export function QuickMessages({
  messages,
  disabled,
  onSend,
}: {
  readonly messages: readonly TriggerQuickMessage[];
  readonly disabled: boolean;
  readonly onSend: (text: string) => void;
}) {
  const [active, setActive] = useState<string>();
  return messages.length === 0 ? null : (
    <div className="agent-quick-messages" role="group" aria-label="Trigger quick messages">
      {messages.map((message) => (
        <QuickMessage
          key={message.name}
          message={message}
          disabled={disabled}
          active={active === message.name}
          onActivate={() => setActive(message.name)}
          onSend={onSend}
        />
      ))}
    </div>
  );
}

function QuickMessage({
  message,
  disabled,
  active,
  onActivate,
  onSend,
}: {
  readonly message: TriggerQuickMessage;
  readonly disabled: boolean;
  readonly active: boolean;
  readonly onActivate: () => void;
  readonly onSend: (text: string) => void;
}) {
  const id = useId();
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const anchor = useRef<HTMLSpanElement>(null);
  const panel = useRef<HTMLSpanElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const visible = active && (hovered || focused) && !dismissed;
  const enter = () => {
    clearTimeout(timer.current);
    setHovered(true);
    setDismissed(false);
    onActivate();
  };
  const leave = () => {
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setHovered(false), 150);
  };
  useEffect(() => () => clearTimeout(timer.current), []);
  useLayoutEffect(() => {
    if (!visible) return;
    const place = () => {
      const row = anchor.current?.parentElement;
      const element = panel.current;
      if (row == null || element === null) return;
      element.style.width = `${Math.min(320, row.clientWidth, innerWidth - 24)}px`;
      const position = placeMenu(
        row.getBoundingClientRect(),
        { width: element.offsetWidth, height: element.offsetHeight },
        { width: innerWidth, height: innerHeight },
        'top',
        'start',
      );
      element.style.left = `${position.left}px`;
      element.style.top = `${position.top}px`;
    };
    place();
    const resize = new ResizeObserver(place);
    if (anchor.current?.parentElement) resize.observe(anchor.current.parentElement);
    window.addEventListener('resize', place);
    window.addEventListener('scroll', place, true);
    return () => {
      resize.disconnect();
      window.removeEventListener('resize', place);
      window.removeEventListener('scroll', place, true);
    };
  }, [visible, message.description]);
  return (
    <span ref={anchor} className="agent-quick-message" onMouseEnter={enter} onMouseLeave={leave}>
      <Button
        variant="outline"
        size="sm"
        disabled={disabled}
        aria-describedby={id}
        onFocus={() => {
          onActivate();
          setFocused(true);
          setDismissed(false);
        }}
        onBlur={() => setFocused(false)}
        onKeyDown={(event) => {
          if (event.key === 'Escape') setDismissed(true);
        }}
        onClick={() => {
          setDismissed(true);
          onSend(message.text);
        }}
      >
        {message.name}
      </Button>
      {createPortal(
        <span
          ref={panel}
          id={id}
          role="tooltip"
          className="agent-quick-message__description"
          hidden={!visible}
          onMouseEnter={enter}
          onMouseLeave={leave}
        >
          {message.description}
        </span>,
        document.body,
      )}
    </span>
  );
}
