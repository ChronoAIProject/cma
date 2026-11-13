import type { UsageClient } from './client';
import { Modal } from '../ui/Modal';
import { MetricsDashboard } from './metrics/MetricsDashboard';
import { useMeasuredUsage } from './metrics/useMeasuredUsage';
import { useUsageSelection } from './metrics/selection';
import { UsageAgentPicker } from './UsageAgentPicker';
import type { DocumentsClient } from '../api/documents';
import { DocumentUsage } from '../documents/DocumentUsage';

export function UsageDialog({
  client,
  open,
  onClose,
  appearanceNotice = false,
  documents,
}: {
  readonly client?: UsageClient;
  readonly open: boolean;
  readonly onClose: () => void;
  readonly appearanceNotice?: boolean;
  readonly documents?: Pick<DocumentsClient, 'documentUsage'>;
}) {
  const measured = useMeasuredUsage(client?.readMeasuredUsage, open);
  const [selection, onSelection] = useUsageSelection(false);
  return (
    <Modal keepMounted open={open} title="Your usage" onClose={onClose} placement="usage-dialog">
      <div className="usage-report">
        {appearanceNotice && <p role="status">Appearance is now in your account menu.</p>}
        {documents ? (
          <section className="usage-metrics" aria-label="Documents usage">
            <h3>Documents</h3>
            <DocumentUsage client={documents} active={open} />
          </section>
        ) : null}
        {client ? (
          <MetricsDashboard
            api={client.usageMetrics}
            history={client.usage}
            admin={false}
            measured={measured}
            active={open}
            selection={selection}
            onSelection={onSelection}
            agentPicker={(draft, update) => (
              <UsageAgentPicker
                client={client}
                sandbox={draft.sandbox}
                value={draft.agent}
                active={open}
                onChange={(agent) => update({ ...draft, agent })}
              />
            )}
          />
        ) : (
          <p role="status">
            Usage reporting is unavailable. Try again when the service is connected.
          </p>
        )}
      </div>
    </Modal>
  );
}
