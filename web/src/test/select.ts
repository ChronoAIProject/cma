import { screen, within } from '@testing-library/react';
import type { UserEvent } from '@testing-library/user-event';

/**
 * Driving the shared dropdown from a test.
 *
 * docs/Web_Style.md §7 gives the product one dropdown pattern, and a native
 * `<select>` cannot draw it — no plate, no flipping caret, no leading-edge
 * mark. So the control is a listbox, and `user.selectOptions` (which only
 * speaks to a real `<select>`) no longer reaches it.
 *
 * These helpers keep the tests asserting the same behaviour they always did:
 * a value is chosen, and the choice is carried through. Only the gesture
 * changes, because only the control did.
 */

/** Open the named dropdown and choose an option by its label. */
export async function chooseOption(
  user: UserEvent,
  dropdown: RegExp | string,
  option: RegExp | string,
): Promise<void> {
  const trigger = screen.getByRole('combobox', { name: dropdown });
  await user.click(trigger);
  await user.click(await screen.findByRole('option', { name: option }));
}

/** What the named dropdown currently reads. */
export function chosenOption(dropdown: RegExp | string): string {
  return screen.getByRole('combobox', { name: dropdown }).textContent ?? '';
}

/** The options the named dropdown offers, once opened. */
export async function openedOptions(
  user: UserEvent,
  dropdown: RegExp | string,
): Promise<readonly string[]> {
  const trigger = screen.getByRole('combobox', { name: dropdown });
  await user.click(trigger);
  const list = await screen.findByRole('listbox', { name: dropdown });
  return within(list)
    .getAllByRole('option')
    .map((option) => option.textContent ?? '');
}
