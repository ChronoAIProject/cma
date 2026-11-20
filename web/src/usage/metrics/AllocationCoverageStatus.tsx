import './AllocationCoverageStatus.css';
import { Badge } from '../../ui/Badge';
import { allocationCoverage, type AllocationCoverageEvidence } from './allocationCoverage';

export function AllocationCoverageStatus({
  value,
}: {
  readonly value: AllocationCoverageEvidence;
}) {
  const history = allocationCoverage(value);
  return (
    <div className="usage-allocation-coverage">
      <details>
        <summary>
          <Badge tone={history.coverage === 'complete' ? 'neutral' : 'warning'} wrap>
            {history.label}
          </Badge>
        </summary>
        <p>{history.description}</p>
      </details>
      {history.coverage === 'missing' && (
        <span>Confirmed missing duration: {history.missingDuration}</span>
      )}
      {history.projection && <span>{history.projection}</span>}
    </div>
  );
}

export function AllocationCoverageNote({
  value,
  scope,
}: {
  readonly value: AllocationCoverageEvidence;
  readonly scope: string;
}) {
  const history = allocationCoverage(value);
  return (
    <div className="usage-metric-coverage" role="note" aria-label={`${scope} coverage`}>
      <p>
        {scope}: {history.label}. {history.description}
      </p>
      {history.projection && <p>{history.projection}</p>}
    </div>
  );
}
