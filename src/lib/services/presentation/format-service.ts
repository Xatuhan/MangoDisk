import * as FormatUtils from '@/lib/utils/presentation/format';

const integerLocale = new Intl.NumberFormat().resolvedOptions().locale;
const dateContexts = new Map<string, { locale: string; timeZone: string }>();
const listLocales = new Map<string, string>();

/** Resolves WebView presentation defaults before calling deterministic formatters. */
export class FormatService {
  private static dateContext(locale?: string): { locale: string; timeZone: string } {
    const key = locale || 'system';
    let context = dateContexts.get(key);
    if (!context) {
      const resolved = new Intl.DateTimeFormat(locale || undefined).resolvedOptions();
      context = { locale: locale || resolved.locale, timeZone: resolved.timeZone };
      dateContexts.set(key, context);
    }
    return context;
  }
  static integer(value: number): string {
    return FormatUtils.integer(value, integerLocale);
  }
  static dateTime(timestamp: number | null | undefined, locale?: string): string {
    const context = FormatService.dateContext(locale);
    return FormatUtils.dateTime(timestamp, context.locale, context.timeZone);
  }
  static date(
    timestamp: number | null | undefined,
    locale?: string,
    precision: 'timestamp' | 'day' = 'timestamp'
  ): string {
    const context = FormatService.dateContext(locale);
    return FormatUtils.date(timestamp, context.locale, precision === 'day' ? 'UTC' : context.timeZone);
  }
  static timeZone(locale?: string): string {
    return FormatService.dateContext(locale).timeZone;
  }
  static list(values: readonly string[], locale?: string): string {
    const key = locale || 'system';
    let resolved = listLocales.get(key);
    if (!resolved) {
      resolved = locale || new Intl.ListFormat().resolvedOptions().locale;
      listLocales.set(key, resolved);
    }
    return FormatUtils.list(values, resolved);
  }
}
