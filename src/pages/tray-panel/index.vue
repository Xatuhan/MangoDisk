<script setup lang="ts">
import { MemoryReleaseService } from '@/lib/services/system-resources/memory-release-service';
import { useMemoryReleaseStore } from '@/stores/memory-release-store';
import { METRIC_STATUS_KEYS } from '@/lib/models/system-resources/system-resources';
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';

import MdIcon from '@/components/icons/md-icon.vue';
import MdTooltip from '@/components/custom/md-tooltip.vue';
import MdMainShortcut from './components/md-main-shortcut.vue';
import { OperatingSystemService } from '@/lib/services/settings/operating-system-service';
import MdResourceOverview from './components/md-resource-overview.vue';
import MdCpuDetails from './components/md-cpu-details.vue';
import MdPowerDetails from './components/md-power-details.vue';
import MdDiskDetails from './components/md-disk-details.vue';
import MdNetworkDetails from './components/md-network-details.vue';
import MdGpuDetails from './components/md-gpu-details.vue';
import MdMemoryOverview from './components/md-memory-overview.vue';
import MdApplicationResourceList from './components/md-application-resource-list.vue';
import { ICON_NAMES } from '@/lib/models/presentation/ui';
import type { ResidentDestination, ResidentPanelPage } from '@/lib/models/resident/resident';
import { ResidentService } from '@/lib/services/resident/resident-service';
import { useTrayPanelStore } from '@/stores/tray-panel-store';
import { useResidentSettingsStore } from '@/stores/resident-settings-store';
import { useWindowsMonitoringStore } from '@/stores/windows-monitoring-store';
import { useAppStore } from '@/stores/app-store';
import { Dialog } from '@/components/ui/dialog';
import MdDialogContent from '@/components/custom/md-dialog-content.vue';
import MdMemoryReleaseSettings from '@/components/memory-release/md-memory-release-settings.vue';
const { t } = useI18n({ useScope: 'global' });
const store = useTrayPanelStore();
const residentSettings = useResidentSettingsStore();
const windowsMonitoring = useWindowsMonitoringStore();
const temperatureEnabled = computed(
  () => !OperatingSystemService.isWindows() || residentSettings.preferences?.windowsCpuTemperatureEnabled === true
);
const temperatureSupported = computed(
  () =>
    !OperatingSystemService.isLinux() &&
    (!OperatingSystemService.isWindows() ||
      (windowsMonitoring.observation !== null && windowsMonitoring.observation.state !== 'unsupportedHardware'))
);
const appStore = useAppStore();
const memorySettings = useMemoryReleaseStore();
const settingsOpen = ref(false);
function setSettingsOpen(open: boolean) {
  if (!memorySettings.saving) settingsOpen.value = open;
}
const memoryReleaseSupported = !OperatingSystemService.isLinux();
const automaticReleaseRule = computed(() => {
  if (!memoryReleaseSupported) return '';
  const preferences = memorySettings.preferences;
  if (!preferences?.automatic) return '';
  const rules = [
    t(preferences.thresholdPercent > 0 ? 'memoryRelease.autoRuleThreshold' : 'memoryRelease.autoRuleAny', {
      minutes: preferences.intervalMinutes,
      percent: preferences.thresholdPercent,
    }),
  ];
  if (OperatingSystemService.isWindows()) {
    if (preferences.skipForeground) rules.push(t('memoryRelease.skipForeground'));
    if (preferences.exclusions.length) {
      rules.push(t('memoryRelease.autoRuleExclusions', { count: preferences.exclusions.length }));
    }
  }
  return rules.join(' · ');
});
const panel = ref<HTMLElement | null>(null);
// Native visibility controls rendering: Windows can reveal an unfocused popup,
// and hiding it does not consistently update document.hidden in WebView2.
const panelVisible = ref(false);
const selectedTab = computed(() =>
  store.selectedPage === 'power' && store.reading.batteryPresent === false
    ? 'cpu'
    : OperatingSystemService.isLinux() && store.selectedPage !== 'cpu' && store.selectedPage !== 'memory'
      ? 'overview'
      : store.selectedPage
);
const tabs = computed<readonly ResidentPanelPage[]>(() =>
  OperatingSystemService.isLinux()
    ? ['overview', 'cpu', 'memory']
    : (['overview', 'cpu', 'gpu', 'memory', 'disk', 'network', 'power'] as const).filter(
        page => page !== 'power' || store.reading.batteryPresent !== false
      )
);
const TAB_DESCRIPTION_KEYS: Record<ResidentPanelPage, string> = {
  overview: 'systemStatus.overview',
  cpu: 'systemStatus.cpu',
  gpu: 'systemStatus.gpu',
  memory: 'systemStatus.memory',
  disk: 'systemStatus.disk',
  network: 'systemStatus.network',
  power: 'power.title',
};
function tabLabel(tab: ResidentPanelPage): string {
  return t(tab === 'cpu' ? 'systemStatus.cpuShort' : TAB_DESCRIPTION_KEYS[tab]);
}
function tabHint(tab: ResidentPanelPage): string | undefined {
  const description = t(TAB_DESCRIPTION_KEYS[tab]);
  return description === tabLabel(tab) ? undefined : description;
}
// Group activity trends before capacity readings without changing native display order.
const overviewMetrics = OperatingSystemService.isLinux()
  ? (['cpu', 'memory', 'disk', 'network'] as const)
  : (['cpu', 'gpu', 'memory', 'disk', 'network'] as const);
// Feedback belongs to this panel's presentation lifecycle. Start its timeout only
// after loading ends; retrying or unmounting cancels the previous result's timer.
watch(
  [() => store.releasing, () => store.releaseResult],
  ([releasing, result], _previous, onCleanup) => {
    if (releasing || !result) return;
    const timer = setTimeout(() => {
      store.releaseResult = null;
    }, 3000);
    onCleanup(() => clearTimeout(timer));
  },
  { immediate: true }
);
let disposed = false;
const disposers: (() => void)[] = [];

function retain(dispose: () => void) {
  if (disposed) dispose();
  else disposers.push(dispose);
}

async function act(action: () => Promise<void>) {
  try {
    await action();
  } catch {
    store.fail('monitoring_action_failed');
  }
}
let metricRevision = 0;
function selectPage(page: ResidentPanelPage) {
  metricRevision += 1;
  store.selectedPage = page;
  void act(() => ResidentService.selectPanelPage(page));
}
// Synchronize native demand as well as rendering when presence arrives after navigation.
watch(
  [() => store.selectedPage, () => store.reading.batteryPresent],
  ([page, present]) => {
    if (page !== 'power' || present !== false) return;
    const restoreFocus = document.activeElement?.closest('#metric-tab-power, [aria-labelledby="metric-tab-power"]');
    selectPage('cpu');
    if (restoreFocus) void nextTick(() => document.getElementById('metric-tab-cpu')?.focus());
  },
  { immediate: true }
);
function selectTab(tab: ResidentPanelPage) {
  selectPage(tab);
}
function moveTab(direction: number) {
  const next = tabs.value[(tabs.value.indexOf(selectedTab.value) + direction + tabs.value.length) % tabs.value.length]!;
  selectTab(next);
  void nextTick(() => document.getElementById(`metric-tab-${next}`)?.focus());
}
function navigate(destination: ResidentDestination) {
  void act(() => ResidentService.openMain(destination));
}
function quit() {
  void act(() => ResidentService.quit());
}
function onKey(event: KeyboardEvent) {
  if (event.key === 'Escape' && !event.defaultPrevented && !settingsOpen.value) {
    if (event.target instanceof Element && event.target.closest('[role="dialog"], [role="menu"], [role="listbox"]'))
      return;
    event.preventDefault();
    void act(() => ResidentService.hidePanel());
  }
}
let subscribed = false;
let connecting: Promise<void> | null = null;
async function connect() {
  if (subscribed || disposed) return;
  if (connecting) return connecting;
  connecting = (async () => {
    // Retrying reconnects the event stream; a one-off cached read cannot restore live updates.
    const pending: (() => void)[] = [];
    try {
      pending.push(
        await ResidentService.onReading(reading => {
          if (!disposed) store.accept(reading);
        })
      );
      pending.push(
        await ResidentService.onPanelPage(metric => {
          if (!disposed) {
            metricRevision += 1;
            store.selectedPage = metric;
          }
        })
      );
      if (!disposed)
        pending.push(
          await ResidentService.onPanelVisibility(visible => {
            if (disposed) return;
            panelVisible.value = visible;
            if (visible && OperatingSystemService.isWindows()) void residentSettings.load();
            if (visible && OperatingSystemService.isWindows()) void windowsMonitoring.load();
            if (!visible) return;
            // A prewarmed WebView survives closing. Do not present an old result
            // as a new action's state when the user returns to the panel.
            if (!store.releasing) store.releaseResult = null;
            void appStore.loadSettings();
            if (memoryReleaseSupported) void memorySettings.load();
            // Background sampling no longer wakes the hidden WebView. Rehydrate
            // from the native cache without waiting for the next sampling tick.
            void store.load();
            panel.value?.focus({ preventScroll: true });
          })
        );
      pending.forEach(retain);
      subscribed = !disposed;
    } catch (error) {
      pending.forEach(dispose => dispose());
      throw error;
    }
  })();
  try {
    await connecting;
  } finally {
    connecting = null;
  }
}
async function refresh() {
  try {
    await connect();
    if (!disposed) await store.refresh();
  } catch {
    store.fail('monitoring_subscription_failed');
  }
}
onMounted(() => {
  // The first native show event can precede WebView subscription. Hydrate settings
  // and capability independently so a missed event cannot leave stale panel state.
  if (OperatingSystemService.isWindows()) {
    void residentSettings.load();
    void windowsMonitoring.load();
  }
  if (memoryReleaseSupported) {
    void MemoryReleaseService.onPreferences(value => memorySettings.accept(value))
      .then(retain)
      .catch(() => {
        memorySettings.failed = true;
      });
    void memorySettings.load();
  }
  window.addEventListener('keydown', onKey);
  // Reveal the first rendered frame independently of IPC, samples, and icons.
  // Native icon components progressively fill their placeholders using the shared cache.
  void act(() => ResidentService.panelReady()).then(() => {
    if (!disposed) panel.value?.focus({ preventScroll: true });
  });
  void connect()
    .then(() => {
      const initialRevision = metricRevision;
      if (!disposed)
        return Promise.all([
          store.load(),
          ResidentService.panelPage().then(metric => {
            if (!disposed && initialRevision === metricRevision) store.selectedPage = metric;
          }),
        ]);
    })
    .catch(() => {
      if (!disposed) store.fail('monitoring_subscription_failed');
    });
});
onBeforeUnmount(() => {
  disposed = true;
  window.removeEventListener('keydown', onKey);
  disposers.forEach(dispose => dispose());
});
</script>

<template>
  <main ref="panel" class="monitor-panel" tabindex="-1" :aria-label="t('monitoring.title')">
    <div class="monitor-body">
      <div class="resource-header">
        <div class="resource-tabs" role="tablist" :aria-label="t('systemStatus.details')">
          <MdTooltip v-for="tab in tabs" :key="tab" :text="tabHint(tab)" :active="panelVisible">
            <button
              :id="`metric-tab-${tab}`"
              role="tab"
              :aria-label="t(TAB_DESCRIPTION_KEYS[tab])"
              :aria-selected="selectedTab === tab"
              aria-controls="metric-details"
              :tabindex="selectedTab === tab ? 0 : -1"
              @click="selectTab(tab)"
              @keydown.right.prevent="moveTab(1)"
              @keydown.left.prevent="moveTab(-1)"
            >
              {{ tabLabel(tab) }}
            </button>
          </MdTooltip>
        </div>
      </div>
      <MdPowerDetails
        v-if="selectedTab === 'power'"
        id="metric-details"
        role="tabpanel"
        aria-labelledby="metric-tab-power"
        :reading="store.reading"
        :active="panelVisible"
        :system-power-supported="OperatingSystemService.isMacOs()"
      />
      <section
        v-if="selectedTab === 'overview'"
        id="metric-details"
        class="resource-cards"
        role="tabpanel"
        aria-labelledby="metric-tab-overview"
      >
        <MdResourceOverview
          v-for="metric in overviewMetrics"
          :key="metric"
          :show-cpu-temperature="temperatureEnabled"
          class="detail-summary"
          :active="panelVisible"
          :metric="metric"
          :interactive="!OperatingSystemService.isLinux() || metric === 'cpu' || metric === 'memory'"
          :reading="store.reading"
          @cleanup="navigate('cleanup')"
          @memory="selectTab('memory')"
          @cpu="selectTab('cpu')"
          @gpu="selectTab('gpu')"
          @disk="selectTab('disk')"
          @network="selectTab('network')"
        />
      </section>
      <section
        v-if="selectedTab === 'gpu'"
        id="metric-details"
        class="resource-details"
        role="tabpanel"
        aria-labelledby="metric-tab-gpu"
      >
        <MdGpuDetails :reading="store.reading" :active="panelVisible" />
      </section>
      <section
        v-if="selectedTab === 'disk' || selectedTab === 'network'"
        id="metric-details"
        class="resource-details"
        role="tabpanel"
        :aria-labelledby="`metric-tab-${selectedTab}`"
      >
        <MdDiskDetails
          v-if="selectedTab === 'disk'"
          :reading="store.reading"
          :active="panelVisible"
          @cleanup="navigate('cleanup')"
        />
        <MdNetworkDetails v-else :reading="store.reading" :active="panelVisible" />
      </section>
      <div v-if="store.error" class="monitor-notice" role="alert">
        {{ t('monitoring.unavailable') }} <button @click="refresh()">{{ t('monitoring.refresh') }}</button>
      </div>
      <section
        v-if="selectedTab === 'cpu'"
        id="metric-details"
        class="resource-details"
        role="tabpanel"
        aria-labelledby="metric-tab-cpu"
      >
        <MdCpuDetails
          :reading="store.reading"
          :active="panelVisible"
          :temperature-enabled="temperatureEnabled"
          :temperature-supported="temperatureSupported"
          @configure-temperature="navigate('displaySettings')"
        />
        <MdApplicationResourceList
          class="monitor-processes"
          metric="cpu"
          :active="panelVisible"
          :summary="store.reading.cpuProcesses.value"
          :status="store.reading.cpuProcesses.status"
        />
      </section>
      <section
        v-if="store.selectedPage === 'memory'"
        id="metric-details"
        class="resource-details"
        role="tabpanel"
        aria-labelledby="metric-tab-memory"
      >
        <template v-if="store.reading.memory.value">
          <span v-if="store.reading.memory.status !== 'ready'" class="metric-stale" role="status">{{
            t(METRIC_STATUS_KEYS[store.reading.memory.status])
          }}</span>
          <MdMemoryOverview
            class="detail-summary"
            :memory="store.reading.memory.value.memory"
            :status="store.reading.memory.status"
            :releasing="store.releasing"
            :release-result="store.releaseResult"
            :release-available="memoryReleaseSupported"
            :active="panelVisible"
            :automatic-release="memorySettings.preferences?.automatic ?? null"
            :release-settings-failed="memorySettings.failed"
            :automatic-release-rule="automaticReleaseRule"
            @release="store.releaseMemory()"
            @settings="settingsOpen = true"
            @reload-settings="memorySettings.load()"
          />
          <MdApplicationResourceList
            class="monitor-processes"
            :active="panelVisible"
            :summary="store.reading.memoryProcesses.value"
            :status="store.reading.memoryProcesses.status"
          />
        </template>
        <div v-else class="monitor-loading" role="status">
          {{ t(METRIC_STATUS_KEYS[store.reading.memory.status]) }}
        </div>
      </section>
    </div>
    <Dialog :open="settingsOpen" @update:open="setSettingsOpen">
      <MdDialogContent
        v-if="settingsOpen"
        size="compact"
        :show-close="!memorySettings.saving"
        :aria-describedby="undefined"
        @escape-key-down="
          event => {
            if (memorySettings.saving) event.preventDefault();
          }
        "
        @pointer-down-outside="event => event.preventDefault()"
        @close-auto-focus="
          event => {
            event.preventDefault();
            panel?.focus({ preventScroll: true });
          }
        "
      >
        <MdMemoryReleaseSettings
          dialog
          :preferences="memorySettings.preferences"
          :saving="memorySettings.saving"
          :failed="memorySettings.failed"
          :reload-preferences="memorySettings.load"
          :save-preferences="memorySettings.save"
          @close="setSettingsOpen(false)"
        />
      </MdDialogContent>
    </Dialog>
    <footer>
      <button class="panel-icon-button" :aria-label="t('monitoring.settings')" @click="navigate('settings')">
        <MdIcon :name="ICON_NAMES.settings" :size="16" />
      </button>
      <MdMainShortcut @error="store.fail('monitoring_action_failed')" />
      <button class="quit-shortcut" @click="quit">
        {{ t('monitoring.quit') }}
      </button>
    </footer>
  </main>
</template>

<style scoped>
@reference "@assets/main.css";
.resource-cards {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  grid-template-rows:
    var(--resource-summary-height) var(--resource-summary-height)
    minmax(var(--resource-summary-height), 1fr) minmax(var(--resource-summary-height), 1fr);
  gap: var(--resource-card-gap);
  min-height: 0;
  flex: 1;
  overflow-y: auto;
}
.resource-header {
  @apply border-b border-border;
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 4px 12px;
  flex: none;
}
.resource-tabs {
  display: flex;
  justify-content: flex-start;
  flex-wrap: wrap;
  gap: 4px 14px;
  flex: 1;
  min-width: 0;
  min-height: 28px;
}
.resource-tabs button {
  @apply text-muted-foreground;
  position: relative;
  min-height: 28px;
  padding: 0 0 4px;
  flex: none;
  white-space: nowrap;
  font-size: 12px;
  font-weight: 500;
  background: transparent;
}
.resource-tabs button:hover {
  @apply text-foreground;
  background: transparent;
}
.resource-tabs button[aria-selected='true'] {
  @apply text-primary-text;
}
.resource-tabs button[aria-selected='true']::after {
  /* Overlay the divider so switching tabs never changes the content height. */
  content: '';
  position: absolute;
  bottom: -1px;
  left: 0;
  right: 0;
  height: 2px;
  border-radius: 1px;
  background: var(--primary);
}
.metric-stale {
  @apply text-muted-foreground;
  font-size: 11px;
}
.monitor-panel {
  --resource-summary-height: 104px;
  --resource-card-gap: 8px;
  @apply bg-background text-foreground;
  display: grid;
  grid-template-rows: minmax(0, 1fr) auto;
  height: 100%;
  border: 1px solid var(--border);
  overflow: hidden;
}
.monitor-panel:focus {
  outline: none;
}
.panel-icon-button {
  @apply text-muted-foreground;
  display: inline-grid;
  place-items: center;
  width: 30px;
  height: 30px;
  border-radius: 7px;
  flex: none;
}
button {
  cursor: pointer;
}
button:hover {
  @apply bg-accent text-accent-foreground;
}
button:focus-visible {
  outline: 2px solid var(--ring);
  outline-offset: 2px;
}
button:disabled {
  cursor: default;
  opacity: 0.45;
}
.monitor-body {
  display: flex;
  flex-direction: column;
  gap: 14px;
  overflow: hidden;
  min-height: 0;
  flex: 1;
  padding: 12px 12px 14px;
}
.monitor-processes {
  /* Extend the scroll viewport through the body's right inset to the window edge. */
  margin-right: -12px;
  min-height: 100px;
}
.resource-details {
  display: flex;
  flex-direction: column;
  gap: var(--resource-card-gap);
  min-height: 0;
  flex: 1;
  overflow-y: auto;
  overflow-x: hidden;
  margin-right: -12px;
  padding-right: 12px;
}
.monitor-panel .detail-summary {
  /* Keep summary geometry consistent across overview and detail tabs. */
  height: var(--resource-summary-height);
  box-sizing: border-box;
  display: flex;
  flex-direction: column;
  justify-content: space-between;
  padding: 10px 12px;
}
.resource-cards > .detail-summary:is([data-metric='memory'], [data-metric='disk'], [data-metric='network']) {
  grid-column: 1 / -1;
  /* CPU and GPU share a row; full-width capacity and activity cards retain their geometry. */
  height: auto;
  min-height: var(--resource-summary-height);
  flex: 1;
}
.resource-cards:not(:has([data-metric='gpu'])) > [data-metric='cpu'] {
  grid-column: 1 / -1;
}
.detail-summary :deep(.resource-trend) {
  height: var(--resource-trend-height, 28px);
}
.monitor-loading {
  @apply text-muted-foreground;
  display: grid;
  place-items: center;
  min-height: 200px;
  font-size: 12px;
}
.monitor-notice {
  @apply border border-border rounded-lg text-muted-foreground;
  flex: none;
  font-size: 11px;
  line-height: 1.5;
  padding: 10px;
}
.monitor-notice button {
  @apply text-primary-text;
  text-decoration: underline;
}
footer {
  @apply border-t border-border text-muted-foreground;
  display: grid;
  grid-template-columns: minmax(40px, 1fr) auto minmax(40px, 1fr);
  align-items: center;
  flex: none;
  padding: 6px 14px;
}
.quit-shortcut {
  justify-self: end;
}
.quit-shortcut {
  /* Equal side columns keep the main action centered in every locale. */
  justify-content: center;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  border-radius: 7px;
  min-height: 30px;
  padding: 0 8px;
  font-size: 11px;
}
</style>
