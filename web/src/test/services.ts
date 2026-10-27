import { fireEvent, screen } from '@testing-library/react';

/** Switch service the way a person does: open the service menu, choose a row. */
export function chooseService(name: 'Agent Manager' | 'Event Gateway') {
  fireEvent.click(screen.getByRole('button', { name: /^Service: / }));
  fireEvent.click(screen.getByRole('menuitemradio', { name }));
}

/** The service the visible sidebar says is current. */
export function currentService() {
  return screen.getByRole('button', { name: /^Service: / }).getAttribute('aria-label');
}
