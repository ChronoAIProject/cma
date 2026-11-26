import { AdmissionFields } from './AdmissionFields';
import { strings, type Fields } from './model';

export function SourceAdmissionFields({
  value,
  onChange,
}: {
  value: Fields;
  onChange: (value: Fields) => void;
}) {
  if (
    !['nyxid_relay', 'telegram', 'discord', 'native_lark', 'whatsapp'].includes(String(value.type))
  )
    return null;
  const whatsapp = value.type === 'whatsapp';
  return (
    <AdmissionFields
      value={value.admission}
      onChange={(admission) => onChange({ ...value, admission })}
      allowBotMessages={!['discord', 'native_lark', 'whatsapp'].includes(String(value.type))}
      allowReplyToBot={value.type !== 'native_lark' && !whatsapp}
      privateChats={whatsapp && strings(value.group_ids).length === 0}
      allowGroupMentions={!whatsapp}
      requireAllowedSender={value.type === 'native_lark'}
    />
  );
}
