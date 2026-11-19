import { object } from './channelTopology';

/** Metadata comes from the resource response, never from a local clock. */
export function GatewayRecordMeta({
  createdAt,
  compact = false,
  owner,
}: {
  readonly createdAt?: string;
  readonly compact?: boolean;
  readonly owner?: unknown;
}) {
  const identity = object(owner);
  const creator =
    typeof identity.display_name === 'string'
      ? identity.display_name.trim() || undefined
      : undefined;
  const timestamp = createdAt && Number.isFinite(Date.parse(createdAt)) ? createdAt : undefined;
  const date = timestamp ? new Date(timestamp) : undefined;
  const formattedDate = date
    ? compact
      ? date.toLocaleDateString([], { month: 'short', day: 'numeric' })
      : date.toLocaleString([], {
          month: 'short',
          day: 'numeric',
          hour: '2-digit',
          minute: '2-digit',
        })
    : 'Not reported';
  if (compact)
    return (
      <>
        <span className="sr-only">{`, Created ${formattedDate}`}</span>
        <span className="tree-row__age" aria-hidden="true" title={`Created ${formattedDate}`}>
          {timestamp ? <time dateTime={timestamp}>{formattedDate}</time> : formattedDate}
        </span>
      </>
    );
  return (
    <small className="gateway-record-meta">
      <span title={`Created ${formattedDate}`}>
        Created {timestamp ? <time dateTime={timestamp}>{formattedDate}</time> : formattedDate}
      </span>
      <span title={creator}>By {creator ?? 'Not reported'}</span>
    </small>
  );
}
