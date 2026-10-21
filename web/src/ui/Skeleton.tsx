import './Skeleton.css';

/**
 * A placeholder in the shape of what is loading — `docs/Web_Style.md` §12.1.
 *
 * A bare "Loading…" sentence or a full-screen character leaves the layout to
 * jump when the content arrives; a skeleton reserves its geometry. Decorative
 * only: the owner keeps one polite status saying what is loading.
 */
export function Skeleton({
  width = '100%',
  height = 12,
  radius = 6,
}: {
  readonly width?: number | string;
  readonly height?: number;
  readonly radius?: number;
}) {
  return (
    <span
      className="skeleton skeleton-block"
      aria-hidden="true"
      style={{ width, height, borderRadius: radius }}
    />
  );
}

/** Rows of a list while it loads: a short line over a shorter one, repeated. */
export function SkeletonRows({
  rows = 3,
  label,
}: {
  readonly rows?: number;
  readonly label: string;
}) {
  return (
    <div className="skeleton-rows" role="status" aria-label={label}>
      {Array.from({ length: rows }, (_, index) => (
        <div className="skeleton-rows__row" key={index}>
          <Skeleton width={`${String(72 - ((index * 13) % 30))}%`} height={12} />
          <Skeleton width={`${String(40 - ((index * 7) % 15))}%`} height={10} />
        </div>
      ))}
    </div>
  );
}

/**
 * The console's outline while identity is checked: a sidebar and a page in
 * skeleton, so the frame is already where it will be when the console paints.
 */
export function ShellLoading({ label }: { readonly label: string }) {
  return (
    <div className="shell-loading" role="status" aria-label={label}>
      <div className="shell-loading__rail" aria-hidden="true">
        <Skeleton height={32} radius={8} />
        <Skeleton height={36} radius={8} />
        <Skeleton width="70%" />
        <Skeleton width="55%" />
        <Skeleton width="62%" />
      </div>
      <div className="shell-loading__page" aria-hidden="true">
        <Skeleton width="28%" height={12} />
        <Skeleton width="45%" height={24} radius={8} />
        <Skeleton width="80%" />
        <Skeleton width="66%" />
      </div>
    </div>
  );
}
