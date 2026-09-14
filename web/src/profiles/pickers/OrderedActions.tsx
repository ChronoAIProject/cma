import { Button } from '../../ui/Button';
import {
  IconChevronUpOutline14,
  IconChevronDownOutline14,
  IconTrashOutline16,
} from '../../ui/icons';

export function moveItem<Item>(items: readonly Item[], from: number, to: number): readonly Item[] {
  const next = [...items];
  const [item] = next.splice(from, 1);
  if (item !== undefined) next.splice(to, 0, item);
  return next;
}

export function OrderedActions({
  label,
  index,
  length,
  disabled,
  onMove,
  onRemove,
}: {
  readonly label: string;
  readonly index: number;
  readonly length: number;
  readonly disabled: boolean;
  readonly onMove: (to: number) => void;
  readonly onRemove: () => void;
}) {
  return (
    <div className="profile-order-actions">
      <Button
        type="button"
        size="sm"
        icon={<IconChevronUpOutline14 />}
        title={`Move ${label} up`}
        aria-label={`Move ${label} up`}
        disabled={disabled || index === 0}
        onClick={() => onMove(index - 1)}
      />
      <Button
        type="button"
        size="sm"
        icon={<IconChevronDownOutline14 />}
        title={`Move ${label} down`}
        aria-label={`Move ${label} down`}
        disabled={disabled || index === length - 1}
        onClick={() => onMove(index + 1)}
      />
      <Button
        type="button"
        size="sm"
        icon={<IconTrashOutline16 />}
        title={`Remove ${label}`}
        aria-label={`Remove ${label}`}
        disabled={disabled}
        onClick={onRemove}
      />
    </div>
  );
}
