import { useLayoutEffect, useRef, type ReactNode } from 'react';

export function revealInvalidField(field: HTMLElement) {
  let parent = field.parentElement;
  while (parent) {
    if (parent instanceof HTMLDetailsElement) parent.open = true;
    parent = parent.parentElement;
  }
}

export function AdvancedFields({
  title,
  children,
  enabled = true,
}: {
  title: string;
  children: ReactNode;
  enabled?: boolean;
}) {
  const ref = useRef<HTMLDetailsElement>(null);
  useLayoutEffect(() => {
    if (ref.current?.querySelector(':invalid')) ref.current.open = true;
  });
  if (!enabled) return <>{children}</>;
  return (
    <details
      ref={ref}
      className="channel-advanced"
      onToggle={() => {
        if (ref.current?.querySelector(':invalid')) ref.current.open = true;
      }}
    >
      <summary>{title}</summary>
      <p className="channel-field-hint">Required fields keep this section open until completed.</p>
      {children}
    </details>
  );
}
