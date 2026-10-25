import { describe, expect, it } from 'vitest';
import type { MetricReading, MetricStatus } from '@/lib/models/system-resources/system-resources';
import { cachedMetricValue, hasFreshValue } from './metric-reading';

describe('cached metric presentation', () => {
  it.each(['ready', 'stale', 'failed'] as const)(
    'retains an original %s observation without promoting freshness',
    status => {
      const sample = { status, sampledAtMs: 1000, value: 0 };
      expect(cachedMetricValue(sample, 6001)).toBe(0);
      expect(hasFreshValue(sample, 6001, 5000)).toBe(false);
      expect(sample.sampledAtMs).toBe(1000);
    }
  );
  it.each(['loading', 'disconnected', 'unsupported'] as const)('rejects invalidated %s sources', status => {
    expect(cachedMetricValue({ status, sampledAtMs: 1000, value: 42 }, 2000)).toBeNull();
  });
  it.each([null, NaN, Infinity, -1, 3000])('rejects invalid or future observation time %s', sampledAtMs => {
    expect(cachedMetricValue({ status: 'stale', value: 42, sampledAtMs }, 2000)).toBeNull();
  });
  it('does not invent an observation before the first result', () => {
    for (const status of ['ready', 'stale', 'failed'] as MetricStatus[]) {
      const sample: MetricReading<number> = { status, sampledAtMs: 1000, value: null };
      expect(cachedMetricValue(sample, 2000)).toBeNull();
    }
  });
});

const reading = (): MetricReading<number> => ({ status: 'ready', value: 0, sampledAtMs: 1000 });

describe('metric observation freshness', () => {
  it('accepts zero values and the inclusive expiration boundary', () => {
    expect(hasFreshValue(reading(), 1000, 10_000)).toBe(true);
    expect(hasFreshValue(reading(), 11_000, 10_000)).toBe(true);
    expect(hasFreshValue(reading(), 11_001, 10_000)).toBe(false);
  });

  it.each<MetricStatus>(['loading', 'stale', 'failed', 'unsupported', 'disconnected'])(
    'rejects %s observations independently of their timestamp',
    status => expect(hasFreshValue({ ...reading(), status }, 1000, 10_000)).toBe(false)
  );

  it('rejects missing values, invalid timestamps and backwards clocks', () => {
    expect(hasFreshValue({ ...reading(), value: null }, 1000, 10_000)).toBe(false);
    for (const sampledAtMs of [null, NaN, Infinity, -Infinity]) {
      expect(hasFreshValue({ ...reading(), sampledAtMs }, 1000, 10_000)).toBe(false);
    }
    for (const now of [999, NaN, Infinity, -Infinity]) {
      expect(hasFreshValue(reading(), now, 10_000)).toBe(false);
    }
  });
});
