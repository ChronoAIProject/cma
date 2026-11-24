import { useEffect, useMemo, useRef } from 'react';
import type { ActivityEntry } from './activity';
import { activityDisplayEntries } from './activity';
import './LiveActivity.css';

export function LiveActivity({
  entries,
  syncedAt,
  stale = false,
  loading = false,
}: {
  readonly entries: readonly ActivityEntry[];
  readonly syncedAt?: string;
  readonly stale?: boolean;
  readonly loading?: boolean;
}) {
  const scroll = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const displayed = useMemo(() => activityDisplayEntries(entries), [entries]);
  const last = displayed.at(-1)?.id;
  const hasEntries = displayed.length > 0;
  useEffect(() => {
    const node = scroll.current;
    if (node === null || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(() => {
      if (following.current) node.scrollTop = node.scrollHeight;
    });
    observer.observe(node);
    if (node.firstElementChild !== null) observer.observe(node.firstElementChild);
    return () => observer.disconnect();
  }, [hasEntries]);
  useEffect(() => {
    if (following.current && scroll.current !== null)
      scroll.current.scrollTop = scroll.current.scrollHeight;
  }, [last]);
  return (
    <section className="live-activity" aria-label="Live activity" aria-live="off">
      <div className="live-activity__heading">
        <strong>Live activity</strong>
        {syncedAt === undefined ? null : (
          <span>
            Last synced: <time dateTime={syncedAt}>{new Date(syncedAt).toLocaleTimeString()}</time>
          </span>
        )}
      </div>
      {stale ? (
        <p>Activity updates interrupted. Reconnecting.</p>
      ) : loading ? (
        <p>Syncing recorded activity.</p>
      ) : null}
      <div
        className="live-activity__scroll"
        ref={scroll}
        role="region"
        aria-label="Recorded activity"
        tabIndex={0}
        onScroll={(event) => {
          const node = event.currentTarget;
          following.current = node.scrollHeight - node.scrollTop - node.clientHeight <= 24;
        }}
      >
        {displayed.length === 0 ? (
          <p>No activity recorded yet.</p>
        ) : (
          <ol>
            {displayed.map((entry) => (
              <li key={entry.id}>
                <time dateTime={entry.at} title={new Date(entry.at).toLocaleString()}>
                  {new Date(entry.at).toLocaleTimeString()}
                </time>
                <span>{entry.message}</span>
              </li>
            ))}
          </ol>
        )}
      </div>
    </section>
  );
}
