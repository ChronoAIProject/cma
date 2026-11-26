import lockupDark from './lockup-dark.svg?no-inline';
import lockupLight from './lockup-light.svg?no-inline';
import markDark from './mark-dark.svg?no-inline';
import markLight from './mark-light.svg?no-inline';

export const pixelBrand = {
  lockup: { light: lockupLight, dark: lockupDark, width: 128, height: 40 },
  mark: { light: markLight, dark: markDark, width: 32, height: 32 },
} as const;
