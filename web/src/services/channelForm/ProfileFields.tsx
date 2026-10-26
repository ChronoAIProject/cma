import { Button } from '../../ui/Button';
import { Choice, Section, StringList, TextField } from './Fields';
import { object, optional, records, strings, type Fields } from './model';
import { ValueBuilder } from './ValueBuilder';
export function ProviderFields({
  value,
  onChange,
  immutable = false,
}: {
  value: Fields;
  onChange: (v: Fields) => void;
  immutable?: boolean;
}) {
  const target = object(value.target);
  if (immutable)
    return (
      <Section title="Agent destination">
        <p>Provider: {String(value.slug ?? 'Unavailable')}</p>
        <p>
          {target.type === 'existing_agent'
            ? `Existing agent: ${String(target.agent_id)}`
            : 'Agent created from this channel’s settings'}
        </p>
        <p className="channel-field-hint">
          This destination stays attached to the channel. Create another channel to choose a
          different agent.
        </p>
      </Section>
    );
  return (
    <Section title="Agent destination">
      <TextField
        label="Provider"
        value={value.slug}
        onChange={(slug) => onChange({ ...value, slug })}
        required
      />
      <Choice
        label="Agent target"
        value={target.type ?? 'profile'}
        options={[
          { value: 'profile', label: 'Create an agent from these settings' },
          { value: 'existing_agent', label: 'Use an existing agent' },
        ]}
        onChange={(type) =>
          onChange({ ...value, target: type === 'profile' ? { type } : { type, agent_id: '' } })
        }
      />
      {target.type === 'existing_agent' ? (
        <TextField
          label="Agent ID"
          value={target.agent_id}
          onChange={(agent_id) => onChange({ ...value, target: { ...target, agent_id } })}
          required
        />
      ) : null}
    </Section>
  );
}
export function ProfileFields({
  value,
  onChange,
  immutableIdentity = false,
}: {
  value: Fields;
  onChange: (v: Fields) => void;
  immutableIdentity?: boolean;
}) {
  const metadata = object(value.metadata),
    owner = object(metadata.owner),
    model = object(value.model),
    permission = object(value.permission),
    context = object(value.context);
  const set = (key: string, next: unknown) => onChange({ ...value, [key]: next });
  const setModel = (key: string, next: unknown) => set('model', { ...model, [key]: next });
  const skills = records(context.skills),
    servers = records(context.mcp_servers);
  return (
    <>
      <Section title="Agent settings">
        <TextField
          label="Agent configuration name"
          value={metadata.name}
          onChange={(name) => set('metadata', { ...metadata, name })}
          required
        />
        <TextField
          label="Description"
          value={metadata.description}
          onChange={(description) => set('metadata', { ...metadata, description })}
          multiline
        />
        <fieldset className="channel-form-section" disabled={immutableIdentity}>
          <TextField
            label="Profile provider"
            value={metadata.provider}
            onChange={(provider) => set('metadata', { ...metadata, provider })}
            required
          />
          <TextField
            label="Owner subject"
            value={owner.subject}
            onChange={(subject) => set('metadata', { ...metadata, owner: { ...owner, subject } })}
            required
          />
          <Choice
            label="Owner type"
            value={owner.kind}
            options={['human', 'service_account', 'agent_key', 'organization']}
            onChange={(kind) => set('metadata', { ...metadata, owner: { ...owner, kind } })}
          />
        </fieldset>
        <div className="channel-form-grid">
          <TextField
            label="Model"
            value={model.model}
            onChange={(v) => setModel('model', v)}
            required
          />
          <Choice
            label="Thinking effort"
            value={model.thinking_effort}
            options={['minimal', 'low', 'medium', 'high', 'very_high']}
            onChange={(v) => setModel('thinking_effort', v)}
          />
        </div>
        <Choice
          label="Model access"
          value={model.access}
          options={['managed', 'custom']}
          onChange={(v) => setModel('access', v)}
        />
        <TextField
          label="Model service"
          value={model.service}
          onChange={(v) => setModel('service', v)}
          required
        />
        <Choice
          label="Model API"
          value={model.api_type}
          options={['responses', 'chat_completions']}
          onChange={(v) => setModel('api_type', v)}
        />
        <Choice
          label="Permission mode"
          value={permission.action_mode}
          options={['guarded', 'autonomous']}
          onChange={(action_mode) => set('permission', { ...permission, action_mode })}
        />
        <StringList
          label="Service scope"
          value={strings(permission.service_scope)}
          onChange={(service_scope) => set('permission', { ...permission, service_scope })}
        />
        <TextField
          label="System prompt"
          value={context.system_prompt}
          onChange={(system_prompt) => set('context', { ...context, system_prompt })}
          multiline
        />
      </Section>
      <Section title="Skills">
        {skills.map((skill, index) => (
          <Section key={index} title={`Skill ${index + 1}`}>
            <TextField
              label={`Skill ${index + 1} ID`}
              value={skill.id}
              onChange={(id) =>
                set('context', {
                  ...context,
                  skills: skills.map((v, i) => (i === index ? { ...v, id } : v)),
                })
              }
              required
            />
            <TextField
              label={`Skill ${index + 1} version`}
              value={skill.version}
              onChange={(version) =>
                set('context', {
                  ...context,
                  skills: skills.map((v, i) =>
                    i === index ? optional(v, 'version', version || undefined) : v,
                  ),
                })
              }
            />
            <Button
              onClick={() =>
                set('context', { ...context, skills: skills.filter((_, i) => i !== index) })
              }
            >
              Remove skill {index + 1}
            </Button>
          </Section>
        ))}
        <Button
          disabled={skills.length >= 32}
          onClick={() =>
            set('context', { ...context, skills: [...skills, { source: 'ornn', id: '' }] })
          }
        >
          Add skill
        </Button>
      </Section>
      <Section title="MCP servers">
        {servers.map((server, index) => {
          const auth = object(server.auth),
            update = (next: Fields) =>
              set('context', {
                ...context,
                mcp_servers: servers.map((v, i) => (i === index ? next : v)),
              });
          return (
            <Section key={index} title={`MCP server ${index + 1}`}>
              <TextField
                label={`Server ${index + 1} URL`}
                value={server.url}
                onChange={(url) => update({ ...server, url })}
                required
              />
              <Choice
                label={`Server ${index + 1} authentication`}
                value={auth.type}
                options={['none', 'nyxid_service']}
                onChange={(type) =>
                  update({ ...server, auth: type === 'none' ? { type } : { type, slug: '' } })
                }
              />
              {auth.type === 'nyxid_service' ? (
                <TextField
                  label={`Server ${index + 1} service`}
                  value={auth.slug}
                  onChange={(slug) => update({ ...server, auth: { ...auth, slug } })}
                  required
                />
              ) : null}
              <Button
                onClick={() =>
                  set('context', { ...context, mcp_servers: servers.filter((_, i) => i !== index) })
                }
              >
                Remove server {index + 1}
              </Button>
            </Section>
          );
        })}
        <Button
          disabled={servers.length >= 8}
          onClick={() =>
            set('context', {
              ...context,
              mcp_servers: [...servers, { url: '', auth: { type: 'none' } }],
            })
          }
        >
          Add MCP server
        </Button>
      </Section>
      <Section title="Environment">
        {value.environment != null ? (
          <>
            <ValueBuilder
              label="Environment values"
              value={value.environment}
              onChange={(v) => set('environment', v)}
            />
            <Button onClick={() => onChange(optional(value, 'environment', undefined))}>
              Remove environment override
            </Button>
          </>
        ) : (
          <Button onClick={() => set('environment', {})}>Add environment settings</Button>
        )}
      </Section>
    </>
  );
}
