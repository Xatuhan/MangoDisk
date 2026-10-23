import type { CpuIdentity } from '@/lib/models/system-resources/system-resources';

export function cpuIdentityLabel(identity: CpuIdentity | null): string {
  const model = identity?.model?.trim().replace(/\s+/g, ' ') ?? '';
  const mhz = identity?.nominalFrequencyMhz;
  // Brand strings often include the manufacturer's base clock already.
  if (!mhz || !Number.isFinite(mhz) || mhz < 1 || mhz > 20_000 || /\d+(?:\.\d+)?\s*[GM]Hz\b/i.test(model)) {
    return model;
  }
  const frequency = `${(mhz / 1000).toFixed(1)} GHz`;
  return model ? `${model} · ${frequency}` : frequency;
}
