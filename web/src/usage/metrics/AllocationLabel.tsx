/** Two readable lines; the SVG title and chart data retain the full name. */
export function AllocationLabel({
  x,
  y,
  name,
}: {
  readonly x: number;
  readonly y: number;
  readonly name: string;
}) {
  const words = name.split(/\s+/);
  const lines = [''];
  for (const word of words) {
    const last = lines.length - 1;
    if (lines[last]!.length + word.length + 1 > 20 && lines.length < 2 && lines[last])
      lines.push(word);
    else lines[last] += `${lines[last] ? ' ' : ''}${word}`;
  }
  return (
    <text x={x} y={y} textAnchor="end" fill="var(--dsw-alias-label-secondary)" fontSize={12}>
      <title>{name}</title>
      {lines.map((line, index) => (
        <tspan x={x} dy={index === 0 ? (lines.length === 1 ? 4 : -3) : 14} key={index}>
          {line.length > 20 ? `${line.slice(0, 19)}…` : line}
        </tspan>
      ))}
    </text>
  );
}
