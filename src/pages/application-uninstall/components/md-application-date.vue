<script setup lang="ts">
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';
import MdTooltip from '@/components/custom/md-tooltip.vue';
import type { ApplicationDate, ApplicationDates } from '@/lib/models/applications/application';
import { FormatService } from '@/lib/services/presentation/format-service';

const props = defineProps<{
  value: ApplicationDate | null;
  kind: 'lastUsed' | 'installed';
  readStatus: ApplicationDates['readStatus'];
}>();
const { t, locale } = useI18n({ useScope: 'global' });
const text = computed(() =>
  props.value
    ? FormatService.date(props.value.timestampMs, locale.value, props.value.precision)
    : t(
        props.kind === 'lastUsed'
          ? 'applicationUninstall.lastUsedUnavailable'
          : 'applicationUninstall.installDateUnavailable'
      )
);
const hint = computed(() => {
  if (!props.value) return props.readStatus === 'partial' ? t('applicationUninstall.dateReadIncomplete') : '';
  return [
    t(`applicationUninstall.dateSources.${props.value.source}`),
    props.value.estimated
      ? t(
          props.kind === 'lastUsed'
            ? 'applicationUninstall.usageDateEstimated'
            : 'applicationUninstall.installDateEstimated'
        )
      : '',
    props.value.partial ? t('applicationUninstall.dateReadIncomplete') : '',
  ]
    .filter(Boolean)
    .join('\n');
});
</script>

<template>
  <span class="application-date-value">
    <MdTooltip v-if="hint" :text="hint" multiline>
      <span
        tabindex="0"
        class="application-date-trigger"
        :aria-label="t('applicationUninstall.dateInformation', { date: text })"
        @click.stop
      >
        {{ text }}
      </span>
    </MdTooltip>
    <span v-else>{{ text }}</span>
  </span>
</template>

<style scoped>
@reference "@assets/main.css";
.application-date-value {
  display: inline-flex;
  min-width: 0;
  align-items: center;
  justify-content: flex-end;
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}
.application-date-trigger {
  border-radius: 2px;
}
.application-date-trigger:focus-visible {
  outline: 2px solid var(--ring);
  outline-offset: 2px;
}
</style>
