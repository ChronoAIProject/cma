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
import { FeedbackReasons } from './FeedbackReasons';
import { Modal } from '../ui/Modal';
import { relativeMessageTime, relativeMessageDelay } from './relativeTime';

export function MessageTimestamp({ at }: { readonly at?: string }) {
  const [now, setNow] = useState(Date.now);
  const label = relativeMessageTime(at, now);
  useEffect(() => {
    if (!at || !Number.isFinite(Date.parse(at))) return;
    let timer: ReturnType<typeof setTimeout>;
    const update = () => {
      clearTimeout(timer);
      const current = Date.now();
      setNow(current);
      timer = setTimeout(update, relativeMessageDelay(at, current));
    };
    timer = setTimeout(update, relativeMessageDelay(at, Date.now()));
    window.addEventListener('focus', update);
    document.addEventListener('visibilitychange', update);
    return () => {
      clearTimeout(timer);
      window.removeEventListener('focus', update);
      document.removeEventListener('visibilitychange', update);
    };
  }, [at]);
  return label ? (
    <time className="message__timestamp" dateTime={at} aria-live="off">
      {label}
    </time>
  ) : null;
}

export function MessageFooter({
  id,
  at,
  content,
  committed,
  canVote,
  showVotes = canVote,
  feedback,
}: {
  readonly id: string;
  readonly at?: string;
  readonly content: string;
  readonly committed: boolean;
  readonly canVote: boolean;
  readonly showVotes?: boolean;
  readonly feedback?: MessageFeedbackControls;
}) {
  const [copy, setCopy] = useState<'copied' | 'failed'>();
  useEffect(() => {
    if (copy !== 'copied') return;
    const timer = setTimeout(() => setCopy(undefined), 2500);
    return () => clearTimeout(timer);
  }, [copy]);
  const voting = canVote && committed && feedback !== undefined;
  const row = voting ? feedback.rows[id] : undefined;
  const vote = row?.vote ?? null;
  const unavailable = !canVote
    ? 'Feedback is available for a final answer.'
    : !committed
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
    <>
      <footer
        className="message-footer"
        data-voted={vote ?? undefined}
        data-feedback-status={
          Boolean(copy || row?.pending || row?.saved || row?.error || (voting && feedback.error)) ||
          undefined
        }
      >
        <div className="message-footer__row">
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
            {showVotes ? (
              <>
                <Button
                  size="sm"
                  icon={vote === 'up' ? <IconLikeFill16 /> : <IconLikeOutline16 />}
                  aria-label="Helpful"
                  aria-pressed={vote === 'up'}
                  disabled={unavailable !== undefined}
                  title={unavailable ?? (vote === 'up' ? 'Remove helpful vote' : 'Helpful')}
                  onClick={() => feedback?.setVote(id, vote === 'up' ? null : 'up')}
                />
                <Button
                  size="sm"
                  icon={vote === 'down' ? <IconDislikeFill16 /> : <IconDislikeOutline16 />}
                  aria-label="Needs improvement"
                  aria-pressed={vote === 'down'}
                  disabled={unavailable !== undefined}
                  title={
                    unavailable ??
                    (vote === 'down' ? 'Edit reasons for improvement' : 'Needs improvement')
                  }
                  onClick={() => {
                    if (vote === 'down' && feedback?.reasons) feedback.reasons.open(id);
                    else feedback?.setVote(id, vote === 'down' ? null : 'down');
                  }}
                />
              </>
            ) : null}
          </div>
          <MessageTimestamp key={at ?? 'pending'} at={at} />
          {copy === 'copied' ? (
            <span className="message-footer__status" role="status">
              Copied
            </span>
          ) : null}
          {row?.pending ? (
            <span className="message-footer__status" role="status" aria-label="Vote status">
              Saving vote…
            </span>
          ) : null}
          {row?.saved ? (
            <span className="message-footer__status" role="status" aria-label="Vote status">
              {vote === null ? 'Vote removed' : 'Vote recorded'}
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
        {voting && feedback.error ? (
          <div className="message-footer__error" role="status">
            <span>{feedback.error}</span>
            <Button size="sm" onClick={feedback.reload}>
              Retry feedback
            </Button>
          </div>
        ) : null}
        {voting && feedback.reasons?.denied ? (
          <span role="status">Feedback is unavailable for this account.</span>
        ) : null}
      </footer>
      {voting && feedback.reasons?.rows[id]?.open ? (
        <Modal open title="Reasons for improvement" onClose={() => feedback.reasons?.close(id)}>
          <FeedbackReasons id={id} feedback={feedback} />
        </Modal>
      ) : null}
    </>
  );
}
