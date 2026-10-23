<script setup lang="ts">
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';
import MdSettingsGroup from '@/components/custom/md-settings-group.vue';
import MdSettingsRow from '@/components/custom/md-settings-row.vue';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import type { ResidentPreferences } from '@/lib/models/resident/resident';

const props = defineProps<{ preferences: ResidentPreferences; platform: 'macos' | 'windows' | 'linux' }>();
const emit = defineEmits<{ change: [patch: Partial<ResidentPreferences>] }>();
const { t } = useI18n();
const controlId = computed(() => `${props.platform}-display-mode`);
const value = computed(() =>
  props.platform === 'macos'
    ? props.preferences.macosDisplayMode
    : props.platform === 'linux'
      ? props.preferences.linuxDisplayMode
      : props.preferences.windowsDisplayMode
);
const modes = computed(() =>
  props.platform === 'macos'
    ? (['menuBar', 'floating', 'both'] as const)
    : props.platform === 'linux'
      ? (['tray', 'floating', 'both'] as const)
      : (['taskbar', 'floating', 'both'] as const)
);
function changeMode(value: unknown) {
  if (props.platform === 'macos') {
    if (value === 'menuBar' || value === 'floating' || value === 'both') emit('change', { macosDisplayMode: value });
  } else if (props.platform === 'linux') {
    if (value === 'tray' || value === 'floating' || value === 'both') emit('change', { linuxDisplayMode: value });
  } else if (value === 'taskbar' || value === 'floating' || value === 'both') {
    emit('change', { windowsDisplayMode: value });
  }
}
</script>
<template>
  <MdSettingsGroup plain>
    <MdSettingsRow
      compact
      class="display-mode-row"
      :title="t('floatingMonitor.displayMode')"
      description=""
      :label-for="controlId"
    >
      <Select :model-value="value" @update:model-value="changeMode">
        <SelectTrigger :id="controlId" class="w-auto min-w-32"
          ><SelectValue>{{ t(`floatingMonitor.${value}`) }}</SelectValue></SelectTrigger
        >
        <SelectContent>
          <SelectItem v-for="mode in modes" :key="mode" :value="mode">{{ t(`floatingMonitor.${mode}`) }}</SelectItem>
        </SelectContent>
      </Select>
    </MdSettingsRow>
  </MdSettingsGroup>
</template>
<style scoped>
.display-mode-row.setting-row--compact {
  min-height: 32px;
  padding: 2px 0;
}
.display-mode-row.setting-row--compact :deep(.setting-controls [data-slot='select-trigger']) {
  height: 32px;
}
</style>
