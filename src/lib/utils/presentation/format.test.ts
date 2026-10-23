import { describe, expect, it } from 'vitest';

import * as FormatUtils from '@/lib/utils/presentation/format';
import { BYTE_UNIT_BASES } from '@/lib/utils/presentation/format';

describe('FormatUtils.bytes', () => {
  it('formats the same raw bytes with an explicit decimal or binary base', () => {
    expect(FormatUtils.bytes(10_842_048, BYTE_UNIT_BASES.decimal)).toBe('10.8 MB');
    expect(FormatUtils.bytes(10_842_048, BYTE_UNIT_BASES.binary)).toBe('10.3 MB');
  });

  it('handles empty and invalid values without exposing invalid numbers', () => {
    expect(FormatUtils.bytes(0, BYTE_UNIT_BASES.decimal)).toBe('0 B');
    expect(FormatUtils.bytes(Number.NaN, BYTE_UNIT_BASES.binary)).toBe('0 B');
  });

  it('retains requested precision for a capacity summary', () => {
    expect(FormatUtils.bytes(241_040_000_000, BYTE_UNIT_BASES.decimal, 2)).toBe('241.04 GB');
    expect(FormatUtils.bytes(241_640_000_000, BYTE_UNIT_BASES.decimal, 2)).toBe('241.64 GB');
  });
});

describe('FormatUtils.dateTime', () => {
  it('uses the application locale instead of the host default', () => {
    const timestamp = Date.UTC(2024, 2, 5, 15, 18);
    const chinese = FormatUtils.dateTime(timestamp, 'zh-CN', 'UTC');
    const english = FormatUtils.dateTime(timestamp, 'en-US', 'UTC');

    expect(chinese).toContain('2024');
    expect(chinese.indexOf('03')).toBeLessThan(chinese.indexOf('05'));
    expect(english.indexOf('03')).toBeLessThan(english.indexOf('05'));
    expect(chinese).not.toBe(english);
    expect(english).not.toContain(',');
    expect(english).toBe('03/05/2024 15:18');
  });

  it('returns the empty display text for invalid values', () => {
    expect(FormatUtils.dateTime(null, 'zh-CN', 'UTC')).toBe('—');
    expect(FormatUtils.dateTime(Number.NaN, 'en-US', 'UTC')).toBe('—');
  });
});

describe('explicit formatting context', () => {
  it('formats and sorts calendar dates without fabricating a time of day', () => {
    const timestamp = Date.UTC(2024, 2, 5, 23, 18);
    expect(FormatUtils.date(timestamp, 'en-US', 'Asia/Shanghai')).toBe('03/06/2024');
    expect(FormatUtils.calendarDateKey(timestamp, 'Asia/Shanghai')).toBe(20240306);
    expect(FormatUtils.calendarDateKey(timestamp, 'America/Los_Angeles')).toBe(20240305);
    expect(FormatUtils.date(null, 'en-US', 'UTC')).toBe('—');
  });
  it('uses the supplied time zone across a date boundary', () => {
    const timestamp = Date.UTC(2024, 2, 5, 23, 18);
    expect(FormatUtils.dateTime(timestamp, 'en-US', 'UTC')).toBe('03/05/2024 23:18');
    expect(FormatUtils.dateTime(timestamp, 'en-US', 'Asia/Shanghai')).toBe('03/06/2024 07:18');
    expect(FormatUtils.dateTime(timestamp, 'en-US', 'UTC')).toBe('03/05/2024 23:18');
  });
  it('uses the supplied numeric and list locales independently', () => {
    expect(FormatUtils.integer(1234567, 'en-US')).toBe('1,234,567');
    expect(FormatUtils.integer(1234567, 'de-DE')).toBe('1.234.567');
    expect(FormatUtils.list(['a', 'b'], 'en-US')).toBe(
      new Intl.ListFormat('en-US', { style: 'short', type: 'conjunction' }).format(['a', 'b'])
    );
  });
});
