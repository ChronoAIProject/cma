import { Picker } from '../ui/Picker';
import { Presence } from '../ui/Presence';
import { useEffect, useState } from 'react';
import type { TriggerDocument, TriggerEmbed, TriggerStatistics } from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import { IconEditOutline16, IconPlusOutline16, IconRefreshOutline16 } from '../ui/icons';
import type { TriggerPublisherClient } from './client';
import { BadgePreview } from './BadgePreview';
import { SnippetTabs } from './SnippetTabs';

export function EmbedPanel({
  client,
  document,
  onManageSources,
}: {
  readonly client: TriggerPublisherClient;
  readonly document: TriggerDocument;
  readonly onManageSources: () => void;
}) {
  const placements = document.placements.filter((placement) => placement.retired_at === undefined);
  const [placementId, setPlacementId] = useState(placements[0]?.id ?? '');
  const selectedId = placements.some((placement) => placement.id === placementId)
    ? placementId
    : (placements[0]?.id ?? '');
  const [embedResult, setEmbedResult] = useState<{
    readonly key: string;
    readonly value?: TriggerEmbed;
    readonly failure?: string;
  }>();
  const [statsResult, setStatsResult] = useState<{
    readonly key: string;
    readonly value?: TriggerStatistics;
    readonly failure?: boolean;
  }>();
  const [reload, setReload] = useState(0);
  const embedKey = `${document.id}:${String(document.revision_count)}:${document.availability}:${selectedId}:${String(reload)}`;
  const statsKey = `${document.id}:${String(reload)}`;
  const descriptor = embedResult?.key === embedKey ? embedResult.value : undefined;
  const failure = embedResult?.key === embedKey ? embedResult.failure : undefined;
  const stats = statsResult?.key === statsKey ? statsResult.value : undefined;
  const statsFailure = statsResult?.key === statsKey && statsResult.failure === true;
  useEffect(() => {
    const request = new AbortController();
    if (selectedId !== '' && document.availability === 'published')
      void client.triggers
        .embed(document.id, selectedId, { signal: request.signal })
        .then((value) => {
          if (!request.signal.aborted) setEmbedResult({ key: embedKey, value });
        })
        .catch(() => {
          if (!request.signal.aborted)
            setEmbedResult({ key: embedKey, failure: 'The embed descriptor could not be loaded.' });
        });
    return () => request.abort();
  }, [client, document.id, document.availability, selectedId, embedKey]);
  useEffect(() => {
    const request = new AbortController();
    void client.triggers
      .stats(document.id, { signal: request.signal })
      .then((value) => {
        if (!request.signal.aborted) setStatsResult({ key: statsKey, value });
      })
      .catch(() => {
        if (!request.signal.aborted) setStatsResult({ key: statsKey, failure: true });
      });
    return () => request.abort();
  }, [client, document.id, statsKey]);
  const selectedStats = stats?.placements.find(
    (placement) => placement.placement_id === selectedId,
  );
  return (
    <section className="trigger-embed" aria-labelledby="trigger-embed-title">
      <div className="trigger-section-heading">
        <h2 id="trigger-embed-title">Embed revision {document.revision_count}</h2>
        <Button icon={<IconEditOutline16 />} onClick={onManageSources}>
          Source URLs
        </Button>
      </div>
      {document.availability !== 'published' ? (
        <p>This Trigger is not available for new launches.</p>
      ) : placements.length === 0 ? (
        <div>
          <p>
            Add the HTTPS URL of the page where this button will appear to generate its embed code.
          </p>
          <Button icon={<IconPlusOutline16 />} onClick={onManageSources}>
            Add source URL
          </Button>
        </div>
      ) : (
        <>
          <div className="trigger-field">
            <label htmlFor="embed-placement">Placement</label>
            <Picker
              id="embed-placement"
              value={selectedId}
              onChange={(choice) => setPlacementId(choice)}
              label="Placement"
              options={placements.map((placement) => ({
                value: placement.id,
                label: placement.label || placement.source_url,
              }))}
            />
          </div>
          <>
            {failure === undefined ? (
              descriptor === undefined ? (
                <p role="status">Loading embed</p>
              ) : (
                <>
                  <BadgePreview url={descriptor.badge_url} label={descriptor.label_text} />
                  <SnippetTabs descriptor={descriptor} />
                </>
              )
            ) : null}
            <Presence>
              {failure === undefined ? null : (
                <div role="alert">
                  <p>{failure}</p>
                  <Button
                    icon={<IconRefreshOutline16 />}
                    onClick={() => setReload((value) => value + 1)}
                  >
                    Try again
                  </Button>
                </div>
              )}
            </Presence>
          </>
        </>
      )}
      <dl className="trigger-stats" aria-label="Trigger statistics">
        <div>
          <dt>Launches, all placements</dt>
          <dd>{stats?.totals.launches_total ?? (statsFailure ? 'Unavailable' : 'Loading')}</dd>
        </div>
        <div>
          <dt>Launches, selected placement</dt>
          <dd>
            {selectedId === ''
              ? 'No placement'
              : stats === undefined
                ? statsFailure
                  ? 'Unavailable'
                  : 'Loading'
                : (selectedStats?.launches_total ?? 'Unavailable')}
          </dd>
        </div>
        <div>
          <dt>Agents ready</dt>
          <dd>{stats?.totals.agents_ready_total ?? (statsFailure ? 'Unavailable' : 'Loading')}</dd>
        </div>
      </dl>
      {statsFailure ? (
        <Button icon={<IconRefreshOutline16 />} onClick={() => setReload((value) => value + 1)}>
          Retry statistics
        </Button>
      ) : null}
    </section>
  );
}
