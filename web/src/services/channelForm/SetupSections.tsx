import type { ReactNode } from 'react';
import { AdmissionFields } from './AdmissionFields';
import { ChannelPolicyFields } from './ChannelPolicyFields';
import { Section } from './Fields';
import { SourceAdmissionFields } from './SourceAdmissionFields';
import { ValidationScope } from './ValidationPath';
import { object, records, strings, text, type Fields } from './model';

export function SetupSection({
  title,
  description,
  enabled,
  children,
}: {
  title: string;
  description?: string;
  enabled: boolean;
  children: ReactNode;
}) {
  return enabled ? (
    <Section title={title} description={description}>
      {children}
    </Section>
  ) : (
    <>{children}</>
  );
}

export function SetupPolicy({
  value,
  onChange,
}: {
  value: Fields;
  onChange: (value: Fields) => void;
}) {
  const sources = records(value.sources);
  return (
    <Section
      title="Access & reply policy"
      description="Review who can send messages, which conversations share history, and where replies go before creating the channel."
    >
      {value.connection_ref !== undefined ? (
        <AdmissionFields
          path="/connection_admission"
          label="Connection admission"
          value={value.connection_admission}
          onChange={(connection_admission) => onChange({ ...value, connection_admission })}
        />
      ) : (
        sources.map((source, index) =>
          source.type !== 'custom' ? (
            <Section key={index} title={`Producer ${index + 1} access`}>
              <ValidationScope value={`/sources/${index}`}>
                <SourceAdmissionFields
                  value={source}
                  onChange={(next) =>
                    onChange({
                      ...value,
                      sources: sources.map((item, i) => (i === index ? next : item)),
                    })
                  }
                />
              </ValidationScope>
            </Section>
          ) : null,
        )
      )}
      <ChannelPolicyFields value={value} onChange={onChange} part="policy" />
    </Section>
  );
}

export function SetupReview({ value }: { value: Fields }) {
  const profile = object(value.profile),
    provider = object(value.provider),
    target = object(provider.target);
  const reply = object(value.reply);
  return (
    <Section
      title="Review"
      description="Create channel saves this configuration and connects its provider. It does not create an external bot or NyxID route."
    >
      <dl className="channel-inspector__facts">
        <div>
          <dt>Channel</dt>
          <dd>{text(value.name).trim() || 'Name required'}</dd>
        </div>
        <div>
          <dt>Connection</dt>
          <dd>
            {value.connection_ref !== undefined
              ? text(value.connection_ref) || 'Choose a ready connection'
              : records(value.sources)
                  .map((source) =>
                    source.type === 'nyxid_relay'
                      ? `${text(source.platform)} via NyxID`
                      : text(source.type).replaceAll('_', ' '),
                  )
                  .join(', ')}
          </dd>
        </div>
        <div>
          <dt>Agent destination</dt>
          <dd>
            {target.type === 'existing_agent'
              ? `Existing Agent: ${text(target.agent_id) || 'ID required'}`
              : 'Conversation Agents created from these settings'}
          </dd>
        </div>
        <div>
          <dt>Service scope</dt>
          <dd>
            {strings(object(profile.permission).service_scope).join(', ') || 'No services selected'}
          </dd>
        </div>
        <div>
          <dt>Replies</dt>
          <dd>
            {reply.auto_final !== true
              ? 'Explicit reply tool calls only'
              : records(reply.sinks).length > 0
                ? 'Final answers sent automatically'
                : 'Final answers have no reply destination. Add one or turn off automatic answers.'}
          </dd>
        </div>
      </dl>
      <p className="channel-field-hint">
        Review the visible sender rules and service scope above. Saving still checks connection
        ownership, supported capabilities and provider permissions.
      </p>
    </Section>
  );
}
