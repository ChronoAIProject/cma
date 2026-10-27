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
    typeof owner === 'string'
      ? owner
      : typeof identity.display_name === 'string'
        ? identity.display_name
        : typeof identity.subject === 'string'
          ? identity.subject
          : undefined;
  const timestamp = createdAt && Number.isFinite(Date.parse(createdAt)) ? createdAt : undefined;
  if (compact)
    return (
      <>
        <span className="sr-only">{`, Created ${timestamp ?? 'Not reported'} · By ${creator ?? 'Not reported'}`}</span>
        <span
          className="tree-row__age"
          aria-hidden="true"
          title={`Created ${timestamp ?? 'Not reported'} · By ${creator ?? 'Not reported'}`}
        >
          {timestamp ? (
            <time dateTime={timestamp}>
              {new Date(timestamp).toLocaleDateString([], { month: 'short', day: 'numeric' })}
            </time>
          ) : (
            'Not reported'
          )}
        </span>
      </>
    );
  return (
    <small className="gateway-record-meta">
      <span title={timestamp}>
        Created{' '}
        {timestamp ? (
          <time dateTime={timestamp}>
            {new Date(timestamp).toLocaleString([], {
              month: 'short',
              day: 'numeric',
              hour: '2-digit',
              minute: '2-digit',
            })}
          </time>
        ) : (
          'Not reported'
        )}
      </span>
      <span title={creator}>By {creator ?? 'Not reported'}</span>
    </small>
  );
}
