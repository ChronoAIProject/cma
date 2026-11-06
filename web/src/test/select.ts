import { screen, within } from '@testing-library/react';
import type { UserEvent } from '@testing-library/user-event';

/**
 * Driving the shared dropdown from a test.
 *
 * docs/Web_Style.md §7 gives the product one dropdown pattern, and a native
 * `<select>` cannot draw it. So the control is a button opening a menu, and
 * `user.selectOptions` (which only speaks to a real `<select>`) does not
 * reach it.
 *
 * These helpers keep the tests asserting the same behaviour they always did:
 * a value is chosen, and the choice is carried through. Only the gesture
 * changes, because only the control did — which is exactly why they are
 * written once here rather than in each suite.
 */

/** Open the named dropdown and choose an option by its label. */
export async function chooseOption(
  user: UserEvent,
  dropdown: RegExp | string,
  option: RegExp | string,
): Promise<void> {
  const trigger = screen.getByRole('button', { name: dropdown });
  await user.click(trigger);
  await user.click(await screen.findByRole('menuitemradio', { name: option }));
}

/** What the named dropdown currently reads. */
export function chosenOption(dropdown: RegExp | string): string {
  return screen.getByRole('button', { name: dropdown }).textContent ?? '';
}

/** The options the named dropdown offers, once opened. */
export async function openedOptions(
  user: UserEvent,
  dropdown: RegExp | string,
): Promise<readonly string[]> {
  const trigger = screen.getByRole('button', { name: dropdown });
  await user.click(trigger);
  const list = await screen.findByRole('menu', { name: dropdown });
  return within(list)
    .getAllByRole('menuitemradio')
    .map((option) => option.textContent ?? '');
}

/** Select the exact value through the shared menu's stable option identity. */
export async function chooseOptionValue(
  user: UserEvent,
  trigger: HTMLElement,
  value: string,
): Promise<void> {
  await user.click(trigger);
  const menu = await screen.findByRole('menu', { name: trigger.getAttribute('aria-label') ?? '' });
  const option = within(menu)
    .getAllByRole('menuitemradio')
    .find((item) => item.dataset.id === value);
  if (option === undefined) throw new Error(`Picker option unavailable: ${value}`);
  await user.click(option);
}

/** Expand the persistent sidebar filter disclosure without toggling an already open panel. */
export async function expandNavigationFilters(user: UserEvent): Promise<void> {
  const trigger = await screen.findByRole('button', { name: 'Filter' });
  if (trigger.getAttribute('aria-expanded') !== 'true') await user.click(trigger);
}
