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

  return {
    body: read(
      state === undefined || state === 'brand'
        ? '--dsw-static-lime-400'
        : `--dsw-alias-character-${state}`,
    ),
    lens: read('--dsw-static-zinc-950'),
    sky: read('--dsw-static-zinc-50'),
    ground: read('--dsw-static-zinc-800'),
    key: read('--dsw-static-zinc-50'),
    fill: read('--dsw-static-lime-400'),
    rim: read('--dsw-static-zinc-400'),
    glint: read('--dsw-static-zinc-50'),
    shadow: read('--dsw-static-lime-400'),
  };
}
