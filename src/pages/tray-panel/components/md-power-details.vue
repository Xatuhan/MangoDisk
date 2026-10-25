<script setup lang="ts">
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';
import MdIcon from '@/components/icons/md-icon.vue';
import MdTooltip from '@/components/custom/md-tooltip.vue';
import { ICON_NAMES } from '@/lib/models/presentation/ui';
import {
  METRIC_STATUS_KEYS,
  type MetricReading,
  type ResourceReadings,
} from '@/lib/models/system-resources/system-resources';
import type { BatteryDetails } from '@/lib/models/system-resources/power';
import { cachedMetricValue } from '@/lib/utils/system-resources/metric-reading';
import MdResourceTrend from './md-resource-trend.vue';
import MdResourceCard from './md-resource-card.vue';
import MdResourceFacts from './md-resource-facts.vue';

const props = withDefaults(
  defineProps<{ reading: ResourceReadings; active: boolean; systemPowerSupported?: boolean }>(),
  { systemPowerSupported: true }
);
const { t, locale } = useI18n();
const formatters = computed(() =>
  [0, 1, 2].map(maximumFractionDigits => new Intl.NumberFormat(locale.value, { maximumFractionDigits }))
);
function number(value: number | null | undefined, digits = 1): string {
  return value == null || !Number.isFinite(value) ? '—' : formatters.value[digits]!.format(value);
}
function observed<T>(reading: MetricReading<T>): T | null {
  return cachedMetricValue(reading, props.reading.observedAtMs);
}
const battery = computed(() =>
  props.reading.batteryPresent === false ? null : observed(props.reading.batteryDetails)
);
const system = computed(() => observed(props.reading.systemPower));
const batteryPower = computed(() =>
  props.reading.batteryPresent === false ? null : observed(props.reading.batteryPower)
);
const temperature = computed(() =>
  props.reading.batteryPresent === false ? null : observed(props.reading.batteryTemperature)
);
const STATE_KEYS: Record<BatteryDetails['chargeState'], string> = {
  charging: 'power.states.charging',
  discharging: 'power.states.discharging',
  full: 'power.states.full',
  paused: 'power.states.paused',
  connected: 'power.states.connected',
  unknown: 'power.states.unknown',
};
const CAPACITY_KEYS = { current: 'power.current', fullCharge: 'power.fullCharge', design: 'power.design' };
const batteryState = computed(() => battery.value?.chargeState ?? 'unknown');
const batteryIcon = computed(() => {
  const value = battery.value;
  if (!value) return ICON_NAMES.battery;
  if (value.chargeState === 'charging' && value.externalConnected !== false) return ICON_NAMES.batteryCharging;
  if (value.externalConnected && ['full', 'paused', 'connected'].includes(value.chargeState)) return ICON_NAMES.plug;
  if (value.chargeState !== 'discharging' && value.chargeState !== 'full') return ICON_NAMES.battery;
  const percent = value.chargePercent;
  if (percent === null || !Number.isFinite(percent) || percent <= 0 || percent > 100) return ICON_NAMES.battery;
  if (percent <= 25) return ICON_NAMES.batteryLow;
  return percent <= 75 ? ICON_NAMES.batteryMedium : ICON_NAMES.batteryFull;
});
const time = computed(() => {
  const state = batteryState.value;
  if (state !== 'charging' && state !== 'discharging') return '';
  const seconds = state === 'charging' ? battery.value?.secondsToFull : battery.value?.secondsToEmpty;
  if (seconds == null) return '';
  const minutes = Math.max(1, Math.ceil(seconds / 60));
  const duration =
    minutes < 60
      ? t('power.minutes', { minutes })
      : t('power.hoursMinutes', { hours: Math.floor(minutes / 60), minutes: minutes % 60 });
  return t(state === 'charging' ? 'power.untilFull' : 'power.remaining', { duration });
});
const batteryFacts = computed(() => {
  const capacity = battery.value?.capacity;
  const unit = capacity?.unit === 'milliwattHours' ? 'mWh' : 'mAh';
  return [
    {
      key: 'battery-power',
      label: t(
        batteryPower.value?.state === 'charging'
          ? 'power.intoBattery'
          : batteryPower.value?.state === 'discharging'
            ? 'power.fromBattery'
            : 'power.batteryPower'
      ),
      value: number(batteryPower.value?.watts),
      unit: 'W',
      hint: t('power.batteryPowerHint'),
    },
    {
      key: 'temperature',
      label: t('power.temperature'),
      value: number(temperature.value?.celsius, 0),
      unit: '°C',
    },
    {
      key: 'health',
      label: t('power.health'),
      value: number(battery.value?.healthPercent, 0),
      unit: '%',
      hint: t('power.healthHint'),
    },
    { key: 'cycles', label: t('power.cycles'), value: number(battery.value?.cycleCount, 0), unit: '' },
    { key: 'voltage', label: t('power.batteryVoltage'), value: number(battery.value?.voltageVolts, 2), unit: 'V' },
    {
      key: 'current-ma',
      label: t('power.batteryCurrent'),
      value: number(battery.value?.currentMilliamps, 0),
      unit: 'mA',
    },
    ...(['current', 'fullCharge', 'design'] as const).map(key => ({
      key,
      label: t(CAPACITY_KEYS[key]),
      value: number(capacity?.[key], 0),
      unit: capacity ? unit : '',
    })),
  ];
});
const adapterFacts = computed(() => [
  { key: 'input', label: t('power.inputPower'), value: number(battery.value?.adapter?.inputWatts), unit: 'W' },
  {
    key: 'reported',
    label: t('power.reportedPower'),
    value: number(battery.value?.adapter?.reportedWatts, 0),
    unit: 'W',
    hint: t('power.adapterHint'),
  },
  {
    key: 'voltage',
    label: t('power.adapterVoltage'),
    value: number(battery.value?.adapter?.voltageVolts, 2),
    unit: 'V',
  },
]);
</script>

<template>
  <div class="power-details">
    <header v-if="reading.batteryPresent !== false" class="battery-status">
      <div class="battery-level">
        <MdIcon :name="batteryIcon" :size="22" /><strong
          >{{ number(battery?.chargePercent, 0) }}<small v-if="battery?.chargePercent != null">%</small></strong
        >
      </div>
      <div class="battery-state">
        <span>{{ t(STATE_KEYS[batteryState]) }}</span
        ><small v-if="reading.batteryDetails.status === 'failed'" role="status">{{
          t(METRIC_STATUS_KEYS.failed)
        }}</small
        ><small v-else-if="time">{{ time }}</small>
      </div>
    </header>
    <MdResourceCard v-if="systemPowerSupported && reading.systemPower.status !== 'unsupported'" class="system-power">
      <div class="power-heading">
        <div class="power-label">
          <MdTooltip :text="t('power.systemPowerHint')" :active="active" multiline
            ><span tabindex="0">{{ t('power.systemPower') }}</span></MdTooltip
          ><small>{{ t('power.lastMinute') }}</small>
        </div>
        <strong>{{ number(system?.watts) }}<small v-if="system"> W</small></strong>
      </div>
      <MdResourceTrend
        v-if="reading.systemPowerHistory.length"
        metric="power"
        :history="reading.systemPowerHistory"
        :observed-at-ms="reading.observedAtMs"
        :label="t('power.trendLabel')"
        :active="active"
      />
      <small v-if="!system" class="power-unavailable">{{
        t(METRIC_STATUS_KEYS[reading.systemPower.status === 'ready' ? 'stale' : reading.systemPower.status])
      }}</small>
    </MdResourceCard>
    <template v-if="reading.batteryPresent !== false">
      <section class="power-section battery-facts">
        <h2>{{ t('power.details') }}</h2>
        <MdResourceFacts :facts="batteryFacts" :active="active" />
      </section>
      <section v-if="battery?.externalConnected && battery.adapter" class="power-section adapter-facts">
        <h2>{{ t('power.adapter') }}</h2>
        <MdResourceFacts :facts="adapterFacts" :active="active" />
      </section>
    </template>
    <p v-else-if="!system" class="power-unavailable">{{ t('power.noMeasurements') }}</p>
  </div>
</template>

<style scoped>
@reference "@assets/main.css";
.power-details {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-height: 0;
}
.battery-status {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  min-height: 44px;
  flex: none;
}
.battery-level {
  display: flex;
  align-items: center;
  gap: 8px;
}
.battery-level strong {
  font-size: 28px;
  line-height: 1.2;
  font-variant-numeric: tabular-nums;
}
.battery-level small {
  font-size: 15px;
  margin-left: 3px;
}
.battery-state {
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 3px;
  text-align: right;
  font-size: 12px;
}
.battery-state small,
.power-label small,
.power-unavailable {
  @apply text-muted-foreground;
  font-size: 11px;
}
.power-heading {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  font-size: 12px;
}
.power-label {
  display: flex;
  flex-direction: column;
  gap: 1px;
}
.power-heading strong {
  flex: none;
  font-size: 24px;
  line-height: 1.2;
  font-variant-numeric: tabular-nums;
}
.power-heading strong small {
  font-size: 12px;
  font-weight: normal;
}
.system-power :deep(.resource-trend) {
  height: 26px;
  margin-top: 4px;
}
.power-section {
  flex: none;
}
.power-section h2 {
  @apply text-foreground;
  font-size: 12px;
  font-weight: 600;
  margin-bottom: 4px;
}
/* One shared surface keeps all native facts visible without twelve separate cards. */
.power-section :deep(.resource-facts) {
  @apply rounded-lg bg-muted/40;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 8px 12px;
  padding: 8px 12px;
}
.power-section :deep(.resource-fact) {
  border-radius: 0;
  background: transparent;
  padding: 0;
  display: flex;
  flex-direction: column;
  justify-content: space-between;
  gap: 3px;
}
.power-section :deep(dt) {
  font-size: 11px;
  line-height: 14px;
  align-items: flex-start;
}
.power-section :deep(dt > span:first-child) {
  overflow: visible;
  text-overflow: clip;
  white-space: normal;
  overflow-wrap: anywhere;
}
.power-section :deep(dd) {
  margin: 0;
  font-size: 15px;
  line-height: 20px;
  overflow-wrap: anywhere;
}
.power-unavailable {
  display: block;
  margin-top: 4px;
}
</style>
