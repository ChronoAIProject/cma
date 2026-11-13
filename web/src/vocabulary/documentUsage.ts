const quantity = new Intl.NumberFormat('en-SG', { maximumFractionDigits: 3 });

export const documentQuantity = (value: number): string => quantity.format(value);

export function documentBytes(value: number): string {
  const divisor =
    value >= 2 ** 30 ? 2 ** 30 : value >= 2 ** 20 ? 2 ** 20 : value >= 1024 ? 1024 : 1;
  const unit =
    divisor === 2 ** 30 ? 'GiB' : divisor === 2 ** 20 ? 'MiB' : divisor === 1024 ? 'KiB' : 'B';
  return `${documentQuantity(value / divisor)} ${unit}`;
}
