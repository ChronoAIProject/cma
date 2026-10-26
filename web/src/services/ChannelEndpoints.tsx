import { useId, useRef, useState } from 'react';
import { Button } from '../ui/Button';
import { IconCopyOutline16 } from '../ui/icons';
import type { Channel } from './ChannelEditor';

type Endpoint = keyof Channel['endpoints'];

const ENDPOINTS: readonly (readonly [Endpoint, string, string])[] = [
  ['custom_event_url', 'Event endpoint', 'Post custom events from your source here.'],
  ['nyxid_callback_url', 'Callback endpoint', 'NyxID relays channel messages here.'],
  ['telegram_webhook_url', 'Telegram webhook', 'Telegram posts your bot’s updates here.'],
];

/**
 * A channel's addresses, each with its purpose and a Copy command.
 *
 * Copying a long opaque URL by dragging a selection is where a character gets
 * lost; when the clipboard refuses, the row selects the URL instead so the
 * address can still be copied by hand.
 */
export function ChannelEndpoints({
  endpoints,
}: {
  readonly endpoints: Partial<Channel['endpoints']>;
}) {
  // Visited channels stay mounted side by side, so the heading id is per instance.
  const heading = useId();
  return (
    <section className="gateway-detail__section" aria-labelledby={heading}>
      <h2 id={heading}>Endpoints</h2>
      <ul className="gateway-endpoints">
        {ENDPOINTS.map(([key, name, purpose]) => {
          const url = endpoints[key];
          return url === undefined ? null : (
            <EndpointRow key={key} name={name} purpose={purpose} url={url} />
          );
        })}
      </ul>
    </section>
  );
}

function EndpointRow({
  name,
  purpose,
  url,
}: {
  readonly name: string;
  readonly purpose: string;
  readonly url: string;
}) {
  const [feedback, setFeedback] = useState<'copied' | 'failed'>();
  const address = useRef<HTMLElement>(null);
  // A slower earlier attempt must not overwrite a later attempt's answer.
  const attempt = useRef(0);
  const copy = async () => {
    const current = ++attempt.current;
    try {
      await navigator.clipboard.writeText(url);
      if (current === attempt.current) setFeedback('copied');
    } catch {
      if (current !== attempt.current) return;
      setFeedback('failed');
      const selection = window.getSelection();
      if (address.current !== null && selection !== null)
        selection.selectAllChildren(address.current);
    }
  };
  return (
    <li className="gateway-endpoint">
      <div className="gateway-endpoint__text">
        <strong>{name}</strong>
        <span>{purpose}</span>
        <code ref={address}>{url}</code>
      </div>
      <div className="gateway-endpoint__copy">
        <Button
          size="sm"
          icon={<IconCopyOutline16 />}
          aria-label={`Copy ${name}`}
          onClick={() => void copy()}
        >
          Copy
        </Button>
        <span role="status">
          {feedback === 'copied'
            ? 'Copied'
            : feedback === 'failed'
              ? 'Copy didn’t work. The address is selected to copy by hand.'
              : ''}
        </span>
      </div>
    </li>
  );
}
