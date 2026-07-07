import type { FiatCurrency } from '../state/types';

export function formatInteger(value: number | undefined): string {
  if (value == null || !Number.isFinite(value)) return '—';
  return new Intl.NumberFormat('en-US').format(value);
}

export function formatBytes(value: number | undefined): string {
  if (value == null || !Number.isFinite(value)) return '—';
  if (value < 1024) return `${value} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let next = value / 1024;
  let unit = units[0];
  for (let i = 1; i < units.length && next >= 1024; i += 1) {
    next /= 1024;
    unit = units[i];
  }
  return `${next.toFixed(next >= 10 ? 1 : 2)} ${unit}`;
}

export function formatSats(value: number): string {
  return `${new Intl.NumberFormat('en-US').format(Math.round(value))} sats`;
}

export function formatBtcFromSats(value: number): string {
  return `${(value / 100_000_000).toFixed(8)} BTC`;
}

export function formatFiatFromSats(valueSat: number, ratePerBtc: number, currency: FiatCurrency): string {
  if (!Number.isFinite(ratePerBtc) || ratePerBtc <= 0) return 'Set fiat rate in Settings';
  const value = (valueSat / 100_000_000) * ratePerBtc;
  return new Intl.NumberFormat(undefined, { style: 'currency', currency }).format(value);
}

export function shortHash(hash: string | undefined): string {
  if (!hash) return '—';
  if (hash.length <= 18) return hash;
  return `${hash.slice(0, 9)}…${hash.slice(-9)}`;
}

export function formatDateTime(iso: string): string {
  return new Intl.DateTimeFormat(undefined, {
    year: 'numeric',
    month: 'short',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  }).format(new Date(iso));
}
