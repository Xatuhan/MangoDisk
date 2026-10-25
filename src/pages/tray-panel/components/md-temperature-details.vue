<script setup lang="ts">
import type { TrendPoint } from '@/lib/models/system-resources/system-resources';
import MdResourceTrend from './md-resource-trend.vue';
import MdResourceCard from './md-resource-card.vue';
defineProps<{ history: TrendPoint[]; observedAtMs: number; active: boolean; label: string; identity: string }>();
</script>
<template>
  <MdResourceCard class="temperature-details">
    <slot />
    <div class="temperature-chart">
      <slot name="placeholder">
        <MdResourceTrend
          v-if="history.length"
          :key="identity"
          metric="temperature"
          :history="history"
          :observed-at-ms="observedAtMs"
          :active="active"
          :label="label"
        />
      </slot>
    </div>
  </MdResourceCard>
</template>
<style scoped>
@reference "@assets/main.css";
.temperature-details {
  height: 90px;
  display: flex;
  flex-direction: column;
}
.temperature-chart {
  display: grid;
  align-items: center;
  flex: 1;
  min-height: 0;
}
.temperature-chart :deep(.resource-trend) {
  height: 28px;
  margin-top: 4px;
}
</style>
