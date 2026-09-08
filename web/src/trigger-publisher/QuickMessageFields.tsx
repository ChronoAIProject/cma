import type { TriggerQuickMessage } from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { IconPlusOutline16, IconTrashOutline16 } from '../ui/icons';

export function QuickMessageFields({
  value,
  disabled,
  error,
  onChange,
}: {
  readonly value: readonly TriggerQuickMessage[];
  readonly disabled: boolean;
  readonly error?: string;
  readonly onChange: (value: readonly TriggerQuickMessage[]) => void;
}) {
  const change = (index: number, field: keyof TriggerQuickMessage, text: string) =>
    onChange(
      value.map((message, position) =>
        position === index ? { ...message, [field]: text } : message,
      ),
    );
  return (
    <section className="trigger-quick-messages" aria-label="Quick messages">
      <h3>Quick messages</h3>
      {value.map((message, index) => (
        <fieldset key={index} className="trigger-quick-message" disabled={disabled}>
          <legend>Quick message {index + 1}</legend>
          {(['name', 'description', 'text'] as const).map((field) => (
            <div className="trigger-field" key={field}>
              <label htmlFor={`quick-message-${index}-${field}`}>
                {field === 'name'
                  ? 'Name'
                  : field === 'description'
                    ? 'Description'
                    : 'Message text'}
              </label>
              {field === 'name' ? (
                <Input
                  id={`quick-message-${index}-${field}`}
                  value={message[field]}
                  onChange={(event) => change(index, field, event.target.value)}
                />
              ) : (
                <textarea
                  id={`quick-message-${index}-${field}`}
                  rows={field === 'text' ? 5 : 2}
                  value={message[field]}
                  onChange={(event) => change(index, field, event.target.value)}
                />
              )}
            </div>
          ))}
          <Button
            size="sm"
            icon={<IconTrashOutline16 />}
            aria-label={`Remove quick message ${index + 1}`}
            title={`Remove quick message ${index + 1}`}
            onClick={() => onChange(value.filter((_, position) => position !== index))}
          />
        </fieldset>
      ))}
      {error === undefined ? null : (
        <p className="trigger-error" role="alert">
          {error}
        </p>
      )}
      <Button
        variant="outline"
        icon={<IconPlusOutline16 />}
        disabled={disabled || value.length >= 8}
        onClick={() => onChange([...value, { name: '', description: '', text: '' }])}
      >
        Add quick message
      </Button>
    </section>
  );
}
