import { act, render } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { observeAnimations } from '../test/animations';
import { ServiceSwitchMotion } from './ServiceSwitchMotion';
import { ServiceTabs } from './ServiceTabs';

function Portals({
  gateway = false,
  collapsed = false,
}: {
  gateway?: boolean;
  collapsed?: boolean;
}) {
  return (
    <ServiceSwitchMotion current={gateway ? 'gateway' : 'agent'}>
      <div hidden={gateway}>
        <ServiceTabs route={{ kind: 'agent_manager' }} onNavigate={vi.fn()} collapsed={collapsed} />
      </div>
      <div hidden={!gateway}>
        <ServiceTabs route={{ kind: 'event_gateway' }} onNavigate={vi.fn()} collapsed={collapsed} />
      </div>
    </ServiceSwitchMotion>
  );
}
let motion: ReturnType<typeof observeAnimations>;
beforeEach(() => {
  motion = observeAnimations();
});
afterEach(() => motion.restore());
it('slides only the visible retained indicator between portals and cancels for live reduced motion', () => {
  const view = render(<Portals />);
  expect(motion.records).toHaveLength(0);
  view.rerender(<Portals gateway />);
  expect(motion.records).toHaveLength(1);
  expect(motion.records[0]?.frames).toEqual([
    { transform: 'translateX(0%)' },
    { transform: 'translateX(100%)' },
  ]);
  view.rerender(<Portals />);
  expect(motion.records).toHaveLength(2);
  expect(motion.records[1]?.frames).toEqual([
    { transform: 'translateX(100%)' },
    { transform: 'translateX(0%)' },
  ]);
  act(() => motion.reduce(true));
  expect(motion.records[1]?.animation.cancel).toHaveBeenCalled();
  view.rerender(<Portals gateway />);
  expect(motion.records).toHaveLength(2);
});
it('uses vertical movement in the collapsed service rail', () => {
  const view = render(<Portals collapsed />);
  view.rerender(<Portals gateway collapsed />);
  expect(motion.records[0]?.frames).toEqual([
    { transform: 'translateY(0%)' },
    { transform: 'translateY(100%)' },
  ]);
});
