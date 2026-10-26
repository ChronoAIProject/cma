import { Button } from '../ui/Button';
import { Choice, Section, TextField } from './channelForm/Fields';
import {
  maxOutboundMedia,
  outboundMediaKinds,
  type OutboundMediaDraft,
  type OutboundMediaKind,
  type OutboundNativeAttachment,
} from './outboundMedia';

export function OutboundMediaFields({
  items,
  onChange,
}: {
  readonly items: readonly OutboundMediaDraft[];
  readonly onChange: (items: OutboundMediaDraft[]) => void;
}) {
  const update = (id: string, patch: Partial<OutboundMediaDraft>) =>
    onChange(items.map((item) => (item.id === id ? { ...item, ...patch } : item)));
  return (
    <Section title="Media attachments">
      <p className="channel-field-hint">
        Add up to four public HTTPS media URLs. Message text is optional when media is attached.
      </p>
      {items.map((item, index) => (
        <Section key={item.id} title={`Attachment ${index + 1}`}>
          <div className="channel-form-grid">
            <Choice
              label={`Media type ${index + 1}`}
              value={item.kind}
              options={outboundMediaKinds.map((kind) => ({
                value: kind,
                label: kind.charAt(0).toUpperCase() + kind.slice(1),
              }))}
              onChange={(kind) => update(item.id, { kind: kind as OutboundMediaKind })}
            />
            <TextField
              label={`HTTPS URL ${index + 1}`}
              value={item.url}
              required
              onChange={(url) => update(item.id, { url })}
            />
            <TextField
              label={`Filename ${index + 1} (optional)`}
              value={item.filename}
              onChange={(filename) => update(item.id, { filename })}
            />
            <TextField
              label={`MIME type ${index + 1} (optional)`}
              value={item.mime}
              onChange={(mime) => update(item.id, { mime })}
            />
          </div>
          <TextField
            label={`Caption ${index + 1} (optional)`}
            value={item.caption}
            onChange={(caption) => update(item.id, { caption })}
          />
          <Button
            aria-label={`Remove attachment ${index + 1}`}
            onClick={() => onChange(items.filter((row) => row.id !== item.id))}
          >
            Remove attachment
          </Button>
        </Section>
      ))}
      <Button
        disabled={items.length >= maxOutboundMedia}
        onClick={() =>
          onChange([
            ...items,
            {
              id: crypto.randomUUID(),
              kind: 'image',
              url: '',
              filename: '',
              mime: '',
              caption: '',
            },
          ])
        }
      >
        Add media attachment
      </Button>
    </Section>
  );
}

/** Review text only: remote content is never fetched by opening the inspector. */
export function OutboundMediaReview({
  items,
}: {
  readonly items: readonly OutboundNativeAttachment[];
}) {
  return items.length ? (
    <Section title={`Media attachments (${items.length})`}>
      {items.map((item, index) => (
        <div className="channel-value-field" key={index}>
          <p>
            <strong>
              {index + 1}. {item.kind}
            </strong>
            {item.filename ? ` · ${item.filename}` : ''}
          </p>
          <p>{item.source.url}</p>
          {item.mime_type ? (
            <p className="channel-field-hint">MIME type: {item.mime_type}</p>
          ) : null}
          {item.caption ? <p>Caption: {item.caption}</p> : null}
        </div>
      ))}
    </Section>
  ) : null;
}
