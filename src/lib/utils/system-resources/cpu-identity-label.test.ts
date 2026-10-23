import { describe, expect, it } from 'vitest';
import { cpuIdentityLabel } from '@/lib/utils/system-resources/cpu-identity-label';
describe('CPU identity caption', () => {
  it('preserves the manufacturer clock without duplicating it', () => {
    expect(
      cpuIdentityLabel({ model: 'Intel Core i9 CPU @ 3.70GHz', frequencyClasses: [], nominalFrequencyMhz: 3701 })
    ).toBe('Intel Core i9 CPU @ 3.70GHz');
  });
  it('shows only a known nominal clock and never invents one', () => {
    expect(cpuIdentityLabel({ model: 'Apple M3 Max', frequencyClasses: [], nominalFrequencyMhz: null })).toBe(
      'Apple M3 Max'
    );
    expect(cpuIdentityLabel({ model: 'AMD Ryzen', frequencyClasses: [], nominalFrequencyMhz: 3800 })).toBe(
      'AMD Ryzen · 3.8 GHz'
    );
    expect(cpuIdentityLabel({ model: 'CPU', frequencyClasses: [], nominalFrequencyMhz: Number.NaN })).toBe('CPU');
    expect(cpuIdentityLabel(null)).toBe('');
  });
});
