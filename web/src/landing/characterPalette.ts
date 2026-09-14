import * as THREE from 'three';

export type CharacterState = 'brand' | 'running' | 'completed' | 'pending' | 'failed';

interface CharacterPalette {
  readonly body: THREE.Color;
  readonly lens: THREE.Color;
  readonly sky: THREE.Color;
  readonly ground: THREE.Color;
  readonly key: THREE.Color;
  readonly fill: THREE.Color;
  readonly rim: THREE.Color;
  readonly glint: THREE.Color;
  readonly shadow: THREE.Color;
}

export function readPalette(
  host: HTMLElement,
  state: CharacterState | undefined,
): CharacterPalette {
  const styles = getComputedStyle(host);
  const read = (property: string) => {
    const value = styles.getPropertyValue(property).trim();
    if (value.length === 0) {
      throw new Error(`character colour ${property} did not resolve`);
    }
    return new THREE.Color(value);
  };
  const dark = document.body.hasAttribute('data-ds-dark-theme');

  return {
    body: read(
      state === undefined || state === 'brand'
        ? '--dsw-alias-brand-primary'
        : `--dsw-alias-character-${state}`,
    ),
    lens: read('--dsw-alias-brand-primary-invert'),
    sky: read(dark ? '--dsw-alias-label-primary' : '--dsw-alias-label-primary-foreground'),
    ground: read('--dsw-alias-bg-layer-3'),
    key: read(dark ? '--dsw-alias-label-primary' : '--dsw-alias-label-primary-foreground'),
    fill: read('--dsw-alias-brand-primary'),
    rim: read(dark ? '--dsw-alias-label-secondary' : '--dsw-alias-bg-layer-1'),
    glint: read(dark ? '--dsw-alias-label-primary' : '--dsw-alias-label-primary-foreground'),
    shadow: read(dark ? '--dsw-alias-brand-primary' : '--dsw-alias-label-primary'),
  };
}
