import type { ReactNode } from 'react';
import { useViewArrival } from '../ui/arrivalMotion';

/** Visited portals keep their exact DOM and in-flight work while another portal is visible. */
export function ServiceBody({
  visible = true,
  children,
}: {
  readonly visible?: boolean;
  readonly children: ReactNode;
}) {
  const ref = useViewArrival<HTMLDivElement>('service', visible);
  return (
    <div className="service-shell__body" hidden={!visible} ref={ref}>
      {children}
    </div>
  );
}
