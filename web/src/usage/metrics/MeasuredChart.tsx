import { cloneElement, useEffect, useRef, useState, type ReactElement } from 'react';

export function MeasuredChart({
  className,
  children,
}: {
  readonly className: string;
  readonly children: ReactElement<{ width?: number; height?: number }>;
}) {
  const container = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  useEffect(() => {
    const element = container.current;
    if (!element) return;
    const measure = () => {
      const { clientWidth: width, clientHeight: height } = element;
      setSize((previous) =>
        previous.width === width && previous.height === height ? previous : { width, height },
      );
    };
    measure();
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  return (
    <div ref={container} className={className} aria-hidden="true">
      {size.width > 0 && size.height > 0 ? cloneElement(children, size) : null}
    </div>
  );
}
