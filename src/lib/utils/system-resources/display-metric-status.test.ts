import { describe, expect, it } from 'vitest';
import { readingFixture } from '@/tests/fixtures/resident';
import { displayMetricStatusKey } from './display-metric-status';

const batteryIds = ['batteryTemperature', 'chargingPower', 'batteryPower'] as const;
describe('display metric availability', () => {
  it.each(batteryIds)('reports confirmed absence for %s even when disabled or a previous value survives', id => {
    const reading = readingFixture();
    reading.batteryPresent = false;
    for (const enabled of [true, false]) {
      expect(displayMetricStatusKey(id, enabled, 'windows', reading)).toBe('systemStatus.noBattery');
      expect(displayMetricStatusKey(id, enabled, 'macos', reading)).toBe('systemStatus.noBattery');
    }
  });
  it.each([null, true] as const)(
    'does not infer battery absence from an unsupported sensor with presence=%s',
    present => {
      const reading = readingFixture();
      reading.batteryPresent = present;
      reading.batteryTemperature = { status: 'unsupported', sampledAtMs: null, value: null };
      expect(displayMetricStatusKey('batteryTemperature', false, 'windows', reading)).toBe('systemStatus.unsupported');
    }
  );
  it('explains demand-driven sensors before selection instead of claiming unsupported hardware', () => {
    for (const id of [...batteryIds, 'diskTemperature', 'systemPower', 'fanSpeed'] as const) {
      expect(displayMetricStatusKey(id, false, 'macos', null)).toBe('systemStatus.sampleWhenEnabled');
      expect(displayMetricStatusKey(id, true, 'macos', null)).toBe('systemStatus.loading');
    }
  });
  it('explains expired samples after optional collection is disabled without hiding failures', () => {
    const reading = readingFixture();
    for (const id of [...batteryIds, 'diskTemperature', 'systemPower', 'fanSpeed'] as const) {
      reading[id].status = 'stale';
      expect(displayMetricStatusKey(id, false, 'macos', reading)).toBe('systemStatus.sampleWhenEnabled');
      expect(displayMetricStatusKey(id, true, 'macos', reading)).toBe('systemStatus.stale');
      reading[id].status = 'failed';
      expect(displayMetricStatusKey(id, false, 'macos', reading)).toBe('systemStatus.readFailed');
    }
  });
  it('reports platform support before unknown sampling status', () => {
    expect(displayMetricStatusKey('systemPower', false, 'windows', null)).toBe('systemStatus.platformUnsupported');
    expect(displayMetricStatusKey('systemPower', true, 'windows', readingFixture())).toBe(
      'systemStatus.platformUnsupported'
    );
    for (const id of [
      'gpu',
      'cpuTemperature',
      'gpuTemperature',
      'diskTemperature',
      ...batteryIds,
      'fanSpeed',
    ] as const) {
      expect(displayMetricStatusKey(id, true, 'linux', null)).toBe('systemStatus.platformUnsupported');
    }
  });
  it('distinguishes a failed fan read from a confirmed fanless machine and stopped fan', () => {
    const reading = readingFixture();
    reading.fanSpeed = { status: 'failed', sampledAtMs: 10, value: { fans: [] } };
    expect(displayMetricStatusKey('fanSpeed', true, 'macos', reading)).toBe('systemStatus.readFailed');
    reading.fanSpeed.status = 'unsupported';
    expect(displayMetricStatusKey('fanSpeed', true, 'macos', reading)).toBe('systemStatus.unsupported');
    reading.fanSpeed.status = 'ready';
    expect(displayMetricStatusKey('fanSpeed', true, 'macos', reading)).toBe('systemStatus.noFans');
    reading.fanSpeed.value = { fans: [{ id: 0, rpm: 0 }] };
    expect(displayMetricStatusKey('fanSpeed', true, 'macos', reading)).toBeNull();
  });
  it('reports idle only with a complete positive-duration residency observation', () => {
    const reading = readingFixture();
    reading.cpuIdentity = { model: 'Apple M5', nominalFrequencyMhz: null, frequencyClasses: ['super'] };
    reading.cpuFrequency = {
      status: 'ready',
      sampledAtMs: 1,
      value: {
        averageMhz: null,
        efficiencyMhz: null,
        performanceMhz: null,
        superMhz: null,
        source: 'applePerformanceStates',
        activity: [{ class: 'super', activeRatio: 0, idleRatio: 0.2, downRatio: 0.8, offRatio: 0 }],
      },
    };
    expect(displayMetricStatusKey('cpuSuperFrequency', true, 'macos', reading)).toBe('systemStatus.frequencyIdle');
    reading.cpuFrequency.value!.activity[0]!.downRatio = 0;
    expect(displayMetricStatusKey('cpuSuperFrequency', true, 'macos', reading)).toBe('systemStatus.frequencyNoSample');
    reading.cpuFrequency.status = 'failed';
    expect(displayMetricStatusKey('cpuSuperFrequency', true, 'macos', reading)).toBe('systemStatus.failed');
  });
  it('uses the selected GPU temperature status without accepting another device value', () => {
    const reading = readingFixture();
    expect(displayMetricStatusKey('gpuTemperature', false, 'windows', reading, 'failed')).toBe(
      'systemStatus.readFailed'
    );
    expect(displayMetricStatusKey('gpuTemperature', false, 'windows', reading, 'disconnected')).toBe(
      'systemStatus.disconnected'
    );
    expect(displayMetricStatusKey('gpuTemperature', false, 'windows', reading, 'ready')).toBeNull();
  });
});

it('keeps installed idle frequency classes distinct from missing hardware and expired samples', () => {
  const reading = readingFixture();
  reading.cpuIdentity = {
    model: 'Apple M3 Max',
    nominalFrequencyMhz: null,
    frequencyClasses: ['efficiency', 'performance'],
  };
  reading.cpuFrequency = {
    status: 'ready',
    sampledAtMs: 1000,
    value: {
      averageMhz: null,
      efficiencyMhz: null,
      performanceMhz: 3200,
      superMhz: null,
      source: 'applePerformanceStates',
      activity: [],
    },
  };
  expect(displayMetricStatusKey('cpuEfficiencyFrequency', true, 'macos', reading)).toBe(
    'systemStatus.frequencyNoSample'
  );
  expect(displayMetricStatusKey('cpuPerformanceFrequency', true, 'macos', reading)).toBeNull();
  expect(displayMetricStatusKey('cpuSuperFrequency', true, 'macos', reading)).toBe('systemStatus.unsupported');
  reading.cpuFrequency.status = 'stale';
  expect(displayMetricStatusKey('cpuEfficiencyFrequency', true, 'macos', reading)).toBe('systemStatus.stale');
  expect(displayMetricStatusKey('cpuEfficiencyFrequency', false, 'macos', reading)).toBe(
    'systemStatus.sampleWhenEnabled'
  );
  expect(displayMetricStatusKey('cpuFrequency', true, 'macos', reading)).toBe('systemStatus.platformUnsupported');
});
