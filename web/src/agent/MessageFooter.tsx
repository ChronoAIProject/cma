import { useEffect, useState } from 'react';
import { Button } from '../ui/Button';
import {
  IconCopyOutline16,
  IconLikeOutline16,
  IconLikeFill16,
  IconDislikeOutline16,
  IconDislikeFill16,
} from '../ui/icons';
import type { MessageFeedbackControls } from './useMessageFeedback';

const singaporeTime = new Intl.DateTimeFormat('en-GB', {
  timeZone: 'Asia/Singapore',
  day: '2-digit',
  month: 'short',
  year: 'numeric',
  hour: '2-digit',
  minute: '2-digit',
  hourCycle: 'h23',
});

export function MessageTimestamp({ at }: { readonly at?: string }) {
  const timestamp = at === undefined ? NaN : Date.parse(at);
  return Number.isFinite(timestamp) ? (
    <time className="message__timestamp" dateTime={at}>
      {singaporeTime.format(timestamp)} SGT
    </time>
  ) : (
    <span className="message__timestamp">Time unavailable</span>
  );
}

export function MessageFooter({
  id,
  at,
  content,
  committed,
  canVote,
  feedback,
}: {
  readonly id: string;
  readonly at?: string;
  readonly content: string;
  readonly committed: boolean;
  readonly canVote: boolean;
  readonly feedback?: MessageFeedbackControls;
}) {
  const [copy, setCopy] = useState<'copied' | 'failed'>();
  useEffect(() => {
    if (copy !== 'copied') return;
    const timer = setTimeout(() => setCopy(undefined), 2500);
    return () => clearTimeout(timer);
  }, [copy]);
  const row = canVote ? feedback?.rows[id] : undefined;
  const vote = row?.vote ?? null;
  const unavailable = !committed
    ? 'Available when this message is recorded.'
    : feedback === undefined
      ? 'Only the owner can save message feedback.'
      : !feedback.ready
        ? (feedback.error ?? 'Loading your message feedback…')
        : row?.pending
          ? 'Saving your vote…'
          : row?.error
            ? 'Retry this vote to confirm its outcome before changing it.'
            : undefined;
  return (
    <footer className="message-footer" data-voted={vote ?? undefined}>
      <div className="message-footer__row">
        <MessageTimestamp at={at} />
        <div className="message-footer__actions" role="group" aria-label="Message actions">
          <Button
            size="sm"
            icon={<IconCopyOutline16 />}
            aria-label="Copy content"
            title={copy === 'copied' ? 'Copied' : 'Copy content'}
            disabled={content === ''}
            onClick={() => {
              void (async () => {
                try {
                  if (!navigator.clipboard) throw new Error('Clipboard unavailable.');
                  await navigator.clipboard.writeText(content);
                  setCopy('copied');
                } catch {
                  setCopy('failed');
                }
              })();
            }}
          />
          {canVote ? (
            <>
              <Button
                size="sm"
                icon={vote === 'up' ? <IconLikeFill16 /> : <IconLikeOutline16 />}
                aria-label="Upvote message"
                aria-pressed={vote === 'up'}
                disabled={unavailable !== undefined}
                title={unavailable ?? (vote === 'up' ? 'Remove upvote' : 'Upvote message')}
                onClick={() => feedback?.setVote(id, vote === 'up' ? null : 'up')}
              />
              <Button
                size="sm"
                icon={vote === 'down' ? <IconDislikeFill16 /> : <IconDislikeOutline16 />}
                aria-label="Downvote message"
                aria-pressed={vote === 'down'}
                disabled={unavailable !== undefined}
                title={unavailable ?? (vote === 'down' ? 'Remove downvote' : 'Downvote message')}
                onClick={() => feedback?.setVote(id, vote === 'down' ? null : 'down')}
              />
            </>
          ) : null}
        </div>
        {copy === 'copied' ? (
          <span className="message-footer__status" role="status">
            Copied
          </span>
        ) : null}
        {row?.pending ? (
          <span className="message-footer__status" role="status">
            Saving vote…
          </span>
        ) : null}
      </div>
      {copy === 'failed' ? (
        <span className="message-footer__status" role="status">
          Copy failed. Use Copy content to retry.
        </span>
      ) : null}
      {row?.error ? (
        <div className="message-footer__error" role="status">
          <span>{row.error}</span>
          <Button size="sm" onClick={() => feedback?.setVote(id, row.retryVote ?? null)}>
            Retry vote
          </Button>
        </div>
      ) : null}
    </footer>
  );
}
