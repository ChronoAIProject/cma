import type { FileChange } from '../fileChanges';
export function FileChangeStats({ change }: { readonly change: FileChange }) {
  return (
    <>
      {change.added === undefined || change.removed === undefined ? (
        <span>Counts unavailable</span>
      ) : (
        <>
          <span className="agent-files__added">+{change.added}</span>
          <span className="agent-files__removed">−{change.removed}</span>
        </>
      )}
      <span aria-label="Modified by the Agent">M</span>
    </>
  );
}
