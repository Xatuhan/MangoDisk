import { describe, expect, it } from 'vitest';
import { detailMetric, floatingValue } from './floating-metric';
import type { FloatingEntry } from '@/lib/models/resident/floating-monitor';

function entry(patch: Partial<FloatingEntry>): FloatingEntry {
  return { id: 'cpu', digits: '12', marker: 'C', text: 'CPU 12%', tooltip: 'CPU 12%', tone: 'normal', ...patch };
}
describe('floating metric contracts', () => {
  it('routes auxiliary readings to the matching resource details', () => {
    expect(detailMetric('cpuTemperature')).toBe('cpu');
    expect(detailMetric('gpuTemperature')).toBe('gpu');
    expect(detailMetric('fanSpeed')).toBe('gpu');
    expect(detailMetric('diskTemperature')).toBe('disk');
    expect(detailMetric('upload')).toBe('network');
    expect(detailMetric('download')).toBe('network');
  });
  it('keeps missing readings distinct from zero and carries native units', () => {
    expect(floatingValue(entry({ id: 'fanSpeed', digits: '1602', marker: 'F' }))).toBe('1602');
    expect(floatingValue(entry({ id: 'fanSpeed', digits: '9999' }))).toBe('9999');
    expect(floatingValue(entry({ id: 'fanSpeed', digits: '10000' }))).toBe('10k');
    expect(floatingValue(entry({ id: 'fanSpeed', digits: '12345' }))).toBe('12.3k');
    expect(floatingValue(entry({ id: 'fanSpeed', digits: '30000' }))).toBe('30k');
    expect(floatingValue(entry({ digits: '—' }))).toBe('—');
    expect(floatingValue(entry({ digits: '0' }))).toBe('0%');
    expect(floatingValue(entry({ id: 'download', marker: '↓M' }))).toBe('12MB/s');
    expect(floatingValue(entry({ id: 'upload', marker: '↑B' }))).toBe('12B/s');
    expect(floatingValue(entry({ id: 'chargingPower', marker: 'mW' }))).toBe('12mW');
  });
});

it.each(['cpuFrequency', 'cpuEfficiencyFrequency', 'cpuPerformanceFrequency', 'cpuSuperFrequency'] as const)(
  'keeps the full GHz unit and CPU detail destination for %s',
  id => {
    const entry: FloatingEntry = { id, digits: '3.2', marker: 'CPUP', text: 'CPUP 3.2G', tooltip: '', tone: 'normal' };
    expect(floatingValue(entry)).toBe('3.2GHz');
    expect(floatingValue({ ...entry, digits: '—' })).toBe('—');
    expect(detailMetric(id)).toBe('cpu');
  }
);

it('does not attach a clock unit to a typed idle frequency label', () => {
  expect(
    floatingValue({
      id: 'cpuSuperFrequency',
      digits: '空闲',
      marker: 'CPUS',
      text: '',
      tooltip: '',
      tone: 'normal',
      frequency: { state: 'idle', idleLabel: '空闲' },
    })
  ).toBe('空闲');
});
