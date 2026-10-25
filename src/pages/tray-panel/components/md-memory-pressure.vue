<script setup lang="ts">
import MdTooltip from '@/components/custom/md-tooltip.vue';
import type { MemoryPressure, MetricStatus } from '@/lib/models/system-resources/system-resources';
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';

defineOptions({ inheritAttrs: false });
const props = withDefaults(defineProps<{ pressure: MemoryPressure; status?: MetricStatus; active?: boolean }>(), {
  status: 'ready',
  active: true,
});
const { t } = useI18n({ useScope: 'global' });
const PRESSURE_KEYS: Record<Exclude<MemoryPressure, 'unsupported'> | 'stale', { label: string; hint: string }> = {
  normal: { label: 'monitoring.pressure.normal', hint: 'monitoring.pressure.normalHint' },
  warning: { label: 'monitoring.pressure.warning', hint: 'monitoring.pressure.warningHint' },
  critical: { label: 'monitoring.pressure.critical', hint: 'monitoring.pressure.criticalHint' },
  unavailable: { label: 'monitoring.pressure.unavailable', hint: 'monitoring.pressure.unavailableHint' },
  stale: { label: 'monitoring.pressure.stale', hint: 'monitoring.pressure.staleHint' },
};
const pressureState = computed(() => {
  if (props.pressure === 'unsupported') return null;
  return ['ready', 'stale', 'loading'].includes(props.status) ? props.pressure : 'stale';
});
const pressureHint = computed(() => (pressureState.value ? t(PRESSURE_KEYS[pressureState.value].hint) : undefined));
</script>

<template>
  <MdTooltip v-if="pressureState" :active="active" :text="pressureHint" multiline>
    <button
      v-bind="$attrs"
      type="button"
      class="memory-pressure"
      :data-pressure="pressureState"
      :aria-label="t(PRESSURE_KEYS[pressureState].label)"
    >
      <span class="pressure-dot" aria-hidden="true" />
      <span>{{ t(PRESSURE_KEYS[pressureState].label) }}</span>
    </button>
  </MdTooltip>
</template>

<style scoped>
@reference "@assets/main.css";
.memory-pressure {
  @apply text-muted-foreground;
  display: inline-flex;
  align-items: center;
  gap: 4px;
  min-width: 0;
  font-size: inherit;
  line-height: inherit;
  white-space: nowrap;
  border-radius: 3px;
  cursor: help;
}
.memory-pressure[data-pressure='normal'] {
  color: var(--usage-green);
}
.memory-pressure[data-pressure='warning'] {
  color: var(--warning-foreground);
}
.memory-pressure[data-pressure='critical'] {
  @apply text-destructive-text;
}
.memory-pressure > :last-child {
  overflow: hidden;
  text-overflow: ellipsis;
}
.pressure-dot {
  width: 5px;
  height: 5px;
  flex: none;
  border-radius: 50%;
  background: currentColor;
}
.memory-pressure:focus-visible {
  outline: 2px solid var(--ring);
  outline-offset: 2px;
}
</style>
