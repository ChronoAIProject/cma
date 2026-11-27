import dark16 from './poster-dark-16.png?no-inline';
import dark32 from './poster-dark-32.png?no-inline';
import dark48 from './poster-dark-48.png?no-inline';
import darkSprite from './sprite-dark.png?no-inline';
import light16 from './poster-light-16.png?no-inline';
import light32 from './poster-light-32.png?no-inline';
import light48 from './poster-light-48.png?no-inline';
import lightSprite from './sprite-light.png?no-inline';

export const preparing = {
  frameSize: 48,
  frameDurationsMs: [350, 350, 350],
  posterFrame: 0,
  light: { sprite: lightSprite, posters: { 16: light16, 32: light32, 48: light48 } },
  dark: { sprite: darkSprite, posters: { 16: dark16, 32: dark32, 48: dark48 } },
} as const;
