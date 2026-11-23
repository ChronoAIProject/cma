import { TopographicCanvas } from './TopographicCanvas';
import { topographicEnabled } from './topographicFeature';
import type { TopographicPlacement } from './topographicPolicy';
import license from './vendor/reactdrops-topographic/LICENSE?url&no-inline';
import './TopographicBackdrop.css';

export function TopographicBackdrop({ placement }: { readonly placement: TopographicPlacement }) {
  if (!topographicEnabled()) return null;
  return (
    <div className={`topographic-backdrop topographic-backdrop--${placement}`} aria-hidden="true">
      <link rel="license" href={license} />
      <div className="topographic-backdrop__static" aria-hidden="true" />
      <TopographicCanvas placement={placement} />
    </div>
  );
}
