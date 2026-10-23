import { EMPTY_DISPLAY_TEXT } from '@/lib/models/presentation/ui';
const INTEGER_FORMATTERS = new Map<string, Intl.NumberFormat>();
const LIST_FORMATTERS = new Map<string, Intl.ListFormat>();
const DATE_TIME_FORMATTERS = new Map<string, Intl.DateTimeFormat>();
const DATE_FORMATTERS = new Map<string, Intl.DateTimeFormat>();
export const BYTE_UNIT_BASES = {
  decimal: 1000,
  binary: 1024,
} as const;
export type ByteUnitBase = (typeof BYTE_UNIT_BASES)[keyof typeof BYTE_UNIT_BASES];
const DATE_TIME_FORMAT_OPTIONS: Intl.DateTimeFormatOptions = {
  year: 'numeric',
  month: '2-digit',
  day: '2-digit',
  hour: '2-digit',
  minute: '2-digit',
  hourCycle: 'h23',
};
/**
 * Formats raw bytes with the unit base selected by the presentation adapter.
 *
 * Requiring the base keeps this utility deterministic and leaves environment
 * detection in the service layer. Formatting never changes bytes used by
 * scanning, thresholds, sorting, cleanup plans, history, or release accounting.
 * The optional minimum precision is capped at two digits for compact capacity
 * summaries and does not change existing item-size formatting by default.
 */
export function bytes(bytes: number, unitBase: ByteUnitBase, minimumFractionDigits = 0): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'] as const;
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(unitBase)), units.length - 1);
  const value = bytes / unitBase ** index;
  const defaultDigits = value >= 100 || index === 0 ? 0 : value >= 10 ? 1 : 2;
  const normalizedMinimum = Number.isFinite(minimumFractionDigits)
    ? Math.min(2, Math.max(0, Math.floor(minimumFractionDigits)))
    : 0;
  const digits = Math.max(defaultDigits, normalizedMinimum);
  return `${value.toFixed(digits)} ${units[index]}`;
}
export function dateTime(timestamp: number | null | undefined, locale: string, timeZone: string): string {
  if (timestamp === null || timestamp === undefined || !Number.isFinite(timestamp)) {
    return EMPTY_DISPLAY_TEXT;
  }
  const cacheKey = `${locale}:${timeZone}`;
  let formatter = DATE_TIME_FORMATTERS.get(cacheKey);
  if (!formatter) {
    formatter = new Intl.DateTimeFormat(locale, { ...DATE_TIME_FORMAT_OPTIONS, timeZone });
    DATE_TIME_FORMATTERS.set(cacheKey, formatter);
  }
  return formatter
    .formatToParts(new Date(timestamp))
    .map(part => (part.type === 'literal' ? part.value.replace(/,\s*/g, ' ') : part.value))
    .join('');
}

export function date(timestamp: number | null | undefined, locale: string, timeZone: string): string {
  if (timestamp === null || timestamp === undefined || !Number.isFinite(timestamp)) return EMPTY_DISPLAY_TEXT;
  const key = `${locale}:${timeZone}`;
  let formatter = DATE_FORMATTERS.get(key);
  if (!formatter) {
    formatter = new Intl.DateTimeFormat(locale, { year: 'numeric', month: '2-digit', day: '2-digit', timeZone });
    DATE_FORMATTERS.set(key, formatter);
  }
  return formatter.format(timestamp);
}

/** Numeric calendar keys keep sorting aligned with the day actually displayed by the adapter. */
export function calendarDateKey(timestamp: number, timeZone: string): number {
  if (!Number.isFinite(timestamp)) return 0;
  const key = `calendar:${timeZone}`;
  let formatter = DATE_FORMATTERS.get(key);
  if (!formatter) {
    formatter = new Intl.DateTimeFormat('en-US', { year: 'numeric', month: '2-digit', day: '2-digit', timeZone });
    DATE_FORMATTERS.set(key, formatter);
  }
  const parts = formatter.formatToParts(timestamp);
  const part = (type: Intl.DateTimeFormatPartTypes) => Number(parts.find(value => value.type === type)?.value ?? 0);
  return part('year') * 10000 + part('month') * 100 + part('day');
}
export function elapsedSeconds(startedAtMs: number, finishedAtMs: number): number {
  if (!Number.isFinite(startedAtMs) || !Number.isFinite(finishedAtMs)) {
    return 0;
  }
  return Math.max(0, Math.round((finishedAtMs - startedAtMs) / 1000));
}
export function integer(value: number, locale: string): string {
  let formatter = INTEGER_FORMATTERS.get(locale);
  if (!formatter) {
    formatter = new Intl.NumberFormat(locale);
    INTEGER_FORMATTERS.set(locale, formatter);
  }
  return formatter.format(value);
}
export function list(values: readonly string[], locale: string): string {
  const cacheKey = locale;
  let formatter = LIST_FORMATTERS.get(cacheKey);
  if (!formatter) {
    formatter = new Intl.ListFormat(locale, {
      style: 'short',
      type: 'conjunction',
    });
    LIST_FORMATTERS.set(cacheKey, formatter);
  }
  return formatter.format(values);
}
export function percent(value: number, total: number): number {
  if (total <= 0) return 0;
  return Math.min(100, Math.max(0, (value / total) * 100));
}
