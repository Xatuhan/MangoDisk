import type { ResidentDisplayMetricId } from '@/lib/models/resident/resident';
import type { CpuFrequencyActivity, CpuFrequencyClass } from '@/lib/models/system-resources/system-resources';

export const CPU_FREQUENCY_CLASSES = {
  cpuFrequency: 'average',
  cpuEfficiencyFrequency: 'efficiency',
  cpuPerformanceFrequency: 'performance',
  cpuSuperFrequency: 'super',
} as const satisfies Partial<Record<ResidentDisplayMetricId, CpuFrequencyClass>>;

export type CpuFrequencyDisplayId = keyof typeof CPU_FREQUENCY_CLASSES;
export function isCpuFrequencyDisplay(id: ResidentDisplayMetricId): id is CpuFrequencyDisplayId {
  return Object.hasOwn(CPU_FREQUENCY_CLASSES, id);
}

export function isIdleFrequency(activity: CpuFrequencyActivity | undefined): boolean {
  if (!activity) return false;
  const ratios = [activity.activeRatio, activity.idleRatio, activity.downRatio, activity.offRatio];
  return (
    ratios.every(value => Number.isFinite(value) && value >= 0 && value <= 1) &&
    activity.activeRatio === 0 &&
    Math.abs(activity.idleRatio + activity.downRatio + activity.offRatio - 1) < 0.000001
  );
}
