import { Input } from '../ui/Input';
import { Picker } from '../ui/Picker';
import type { AdmissionChoice } from './model';

export interface PolicyDraft {
  readonly admission: AdmissionChoice;
  readonly senders: string;
  readonly autoFinal: boolean;
  readonly ackSeconds: string;
}
export function BotPolicy({
  value,
  onChange,
}: {
  readonly value: PolicyDraft;
  readonly onChange: (value: PolicyDraft) => void;
}) {
  return (
    <>
      <div className="bots__field">
        <span>Who can talk to this consumer</span>
        <Picker
          label="Admission policy"
          value={value.admission}
          options={[
            { value: '', label: 'Choose who can talk to this consumer' },
            { value: 'private', label: 'Private chats only' },
            { value: 'allowlist', label: 'Selected senders in private chats' },
            { value: 'mention', label: 'Private chats and groups with a mention' },
          ]}
          onChange={(admission) => onChange({ ...value, admission })}
        />
        <p>
          Each sender has a separate conversation in each chat. Group replies are visible to the
          group.
        </p>
      </div>
      {value.admission !== 'allowlist' ? null : (
        <div className="bots__field">
          <label htmlFor="bot-senders">Sender IDs</label>
          <Input
            id="bot-senders"
            value={value.senders}
            onChange={(event) => onChange({ ...value, senders: event.target.value })}
            aria-describedby="bot-senders-help"
          />
          <span id="bot-senders-help">
            Separate IDs with commas. Choose at least one sender; duplicates are not allowed.
          </span>
        </div>
      )}
      <label className="bots__check">
        <input
          type="checkbox"
          checked={value.autoFinal}
          onChange={(event) => onChange({ ...value, autoFinal: event.target.checked })}
        />
        Send the Agent’s final answer automatically
      </label>
      <p>When this is off, the Agent must explicitly send replies using its reply tools.</p>
      <div className="bots__field">
        <label htmlFor="bot-ack">Acknowledgement delay (seconds)</label>
        <Input
          id="bot-ack"
          type="number"
          min="0"
          max="60"
          step="0.001"
          value={value.ackSeconds}
          onChange={(event) => onChange({ ...value, ackSeconds: event.target.value })}
          aria-describedby="bot-ack-help"
        />
        <span id="bot-ack-help">Between 0 and 60 seconds. Zero disables the acknowledgement.</span>
      </div>
    </>
  );
}
