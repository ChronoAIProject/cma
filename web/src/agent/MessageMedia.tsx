import { useContext, useEffect, useRef, useState } from 'react';
import { Button } from '../ui/Button';
import {
  MessageMediaContext,
  messageFilePath,
  messageMediaType,
  safeHttpUrl,
} from './mediaSources';

export function MessageMedia({
  src,
  label,
  image = false,
}: {
  readonly src: string;
  readonly label: string;
  readonly image?: boolean;
}) {
  const access = useContext(MessageMediaContext);
  const remote = safeHttpUrl(src);
  const path = remote ? undefined : messageFilePath(src, access?.workingDirectory);
  const type = messageMediaType(remote ?? path ?? '') ?? (remote && image ? 'image/*' : undefined);
  const container = useRef<HTMLSpanElement>(null);
  const [visible, setVisible] = useState(() => typeof IntersectionObserver === 'undefined');
  const [attempt, setAttempt] = useState(0);
  const [loaded, setLoaded] = useState<{ readonly key: string; readonly url: string }>();
  const [failed, setFailed] = useState<string>();
  const key = `${access?.agentId ?? ''}:${path ?? remote ?? src}:${attempt}`;
  const files = access?.files;
  const agentId = access?.agentId;
  const url = remote ?? (loaded?.key === key ? loaded.url : undefined);

  useEffect(() => {
    if (typeof IntersectionObserver === 'undefined') return;
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) {
        setVisible(true);
        observer.disconnect();
      }
    });
    if (container.current) observer.observe(container.current);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    if (!visible || !path || !files || !agentId || !type) return;
    const abort = new AbortController();
    let objectUrl: string | undefined;
    void files
      .download(agentId, path, { signal: abort.signal })
      .then((blob) => {
        if (abort.signal.aborted) return;
        objectUrl = URL.createObjectURL(new Blob([blob], { type }));
        setLoaded({ key, url: objectUrl });
      })
      .catch(() => {
        if (!abort.signal.aborted) setFailed(key);
      });
    return () => {
      abort.abort();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [agentId, files, path, type, visible, key]);

  if (!type || (!remote && (!path || !access))) return <span>{label || src}</span>;
  return (
    <span className="agent-content__media" ref={container}>
      {failed === key ? (
        <span className="agent-content__media-status" role="status">
          {label || 'Media'} — preview unavailable.
          <Button size="sm" onClick={() => setAttempt((value) => value + 1)}>
            Retry preview
          </Button>
        </span>
      ) : url === undefined ? (
        <span className="agent-content__media-status" role="status">
          Loading {label || 'media'}…
        </span>
      ) : type.startsWith('image/') ? (
        <img
          src={url}
          alt={label}
          loading="lazy"
          referrerPolicy="no-referrer"
          onError={() => setFailed(key)}
        />
      ) : type.startsWith('audio/') ? (
        <audio
          src={url}
          aria-label={label || 'Audio'}
          controls
          preload="none"
          onError={() => setFailed(key)}
        />
      ) : (
        <video
          src={url}
          aria-label={label || 'Video'}
          controls
          preload="metadata"
          onError={() => setFailed(key)}
        />
      )}
      {url === undefined || image ? null : (
        <a
          href={url}
          download={path?.split('/').at(-1)}
          target={remote ? '_blank' : undefined}
          rel="noopener noreferrer"
        >
          {label || 'Open media'}
        </a>
      )}
    </span>
  );
}
