export function WarmPoolImage({
  image,
  version,
}: {
  readonly image: string;
  readonly version: string | undefined;
}) {
  const [reference = '', digest] = image.split('@');
  const name = reference.slice(reference.lastIndexOf('/') + 1);
  const tagSeparator = name.indexOf(':');
  const repository = tagSeparator < 0 ? name : name.slice(0, tagSeparator);
  const tag = tagSeparator < 0 ? undefined : name.slice(tagSeparator + 1);
  const digestLabel = digest?.slice(0, 'sha256:'.length + 12);
  return (
    <details className="warm-pool-config">
      <summary>
        {repository} · {version ?? tag ?? digestLabel ?? 'Version unavailable'}
      </summary>
      <code className="warm-pool-reference">{image}</code>
    </details>
  );
}
