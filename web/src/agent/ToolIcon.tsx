/** Workspace operation symbols use the same currentColor SVG contract as shared icons. */
export function ToolIcon({
  kind,
}: {
  readonly kind:
    | 'command'
    | 'read'
    | 'edit'
    | 'other'
    | 'navigate'
    | 'click'
    | 'drag'
    | 'type'
    | 'key'
    | 'search'
    | 'list';
}) {
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {kind === 'search' ? (
        <>
          <circle cx="7" cy="7" r="4" />
          <path d="m10 10 4 4" />
        </>
      ) : kind === 'list' ? (
        <path d="M1 4V2h5l2 2h7v10H1V4Z" />
      ) : kind === 'command' ? (
        <path d="m2 4 4 4-4 4m6 0h6" />
      ) : kind === 'edit' ? (
        <path d="m3 10 8-8 3 3-8 8-4 1 1-4Zm6-6 3 3" />
      ) : kind === 'navigate' ? (
        <>
          <circle cx="8" cy="8" r="6" />
          <ellipse cx="8" cy="8" rx="2.5" ry="6" />
          <path d="M2 8h12" />
        </>
      ) : kind === 'click' ? (
        <path d="m3 2 9 6-4 1-2 4-3-11Z" />
      ) : kind === 'drag' ? (
        <path d="M8 1v14M1 8h14M5 4l3-3 3 3M5 12l3 3 3-3M4 5 1 8l3 3m8-6 3 3-3 3" />
      ) : kind === 'type' || kind === 'key' ? (
        <>
          <rect x="1" y="3" width="14" height="10" rx="1" />
          <path d="M4 6h1m2 0h1m2 0h1M4 10h8" />
        </>
      ) : (
        <path d="M4 1h5l3 3v11H4V1Zm5 0v4h3" />
      )}
    </svg>
  );
}
