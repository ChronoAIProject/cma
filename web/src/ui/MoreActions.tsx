import { useState } from 'react';
import { Menu, type MenuEntry } from './Menu';
import { Button } from './Button';
import { IconEllipsisOutline16 } from './icons';

/**
 * The overflow for a header's actions — `docs/Web_Style.md` §11.
 *
 * A header holds at most one primary and one secondary action; everything
 * else, including anything destructive, waits here, where an unavailable row
 * keeps its place and says why (§12.3).
 */
export function MoreActions({
  items,
  onSelect,
  label = 'More actions',
}: {
  readonly items: readonly MenuEntry[];
  readonly onSelect: (id: string) => void;
  readonly label?: string;
}) {
  const [open, setOpen] = useState(false);
  if (items.length === 0) return null;
  return (
    <Menu
      open={open}
      align="end"
      label={label}
      items={items}
      onClose={() => setOpen(false)}
      onSelect={(id) => {
        setOpen(false);
        onSelect(id);
      }}
      anchor={
        <Button
          size="sm"
          variant="outline"
          icon={<IconEllipsisOutline16 />}
          aria-label={label}
          aria-haspopup="menu"
          aria-expanded={open}
          onClick={() => setOpen((wasOpen) => !wasOpen)}
        />
      }
    />
  );
}
