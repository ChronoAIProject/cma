import type { ReactNode } from 'react';
import { Table, TableCell } from '../ui/Table';
import type { ChannelListItem } from './ChannelEditor';

const COLUMNS = [
  { header: 'Name' },
  { header: 'Provider' },
  { header: 'Level' },
  { header: 'Sources', align: 'end' },
  { header: 'Version', align: 'end' },
] as const;

/**
 * Every channel at once — `docs/Web_Style.md` §27.
 *
 * The sidebar list disappears behind a collapsed rail on a tablet or phone,
 * which left no way back to the channels; the table is the page's own list
 * and opens a channel exactly as the sidebar does.
 */
export function ChannelTable({
  channels,
  loading,
  unavailable,
  onSelect,
}: {
  readonly channels: readonly ChannelListItem[];
  readonly loading: boolean;
  /** Said in place of the empty sentence when the list could not be read at all. */
  readonly unavailable?: ReactNode;
  readonly onSelect: (recordId: string) => void;
}) {
  return (
    <Table
      label="Your channels"
      placement="gateway-channels"
      columns={COLUMNS}
      loading={loading}
      loadingLabel="Loading channels"
      empty={
        unavailable ?? (
          <>
            <strong className="gateway-channels__empty">No channels yet</strong>
            <span>Connect an event source to your agent.</span>
          </>
        )
      }
    >
      {channels.map((channel) => (
        <tr key={channel.record_id}>
          <TableCell>
            <button
              type="button"
              className="gateway-channels__open"
              onClick={() => onSelect(channel.record_id)}
            >
              {channel.definition.name}
            </button>
          </TableCell>
          <TableCell mono>{channel.provider.slug}</TableCell>
          <TableCell>{channel.provider.level}</TableCell>
          <TableCell align="end">{channel.definition.sources.length}</TableCell>
          <TableCell align="end">{channel.version}</TableCell>
        </tr>
      ))}
    </Table>
  );
}
