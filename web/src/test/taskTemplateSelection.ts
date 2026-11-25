import { fireEvent, screen } from '@testing-library/react';

export function selectTaskTemplate(name: string) {
  const expanded = screen.queryByRole('button', { name });
  if (expanded) {
    fireEvent.click(expanded);
    return;
  }
  fireEvent.click(screen.getByRole('button', { name: 'Templates' }));
  fireEvent.click(screen.getByRole('menuitem', { name }));
}
