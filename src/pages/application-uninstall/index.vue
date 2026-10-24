<script setup lang="ts">
import { FormatService } from '@/lib/services/presentation/format-service';
import MdAiWorkspace from '@/layouts/components/md-ai-workspace.vue';
import { useAiStore } from '@/stores/ai-store';
import { applicationUninstallAiContext } from './application-uninstall-ai-context';
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { toast } from 'vue-sonner';

import MdCategoryFilter from '@/components/custom/md-category-filter.vue';
import MdEmptyState from '@/components/custom/md-empty-state.vue';
import MdOperationProgress from '@/components/custom/md-operation-progress.vue';
import MdOperationWorkspace from '@/components/custom/md-operation-workspace.vue';
import MdPageShell from '@/components/custom/md-page-shell.vue';
import MdResultCheckbox from '@/components/custom/md-result-checkbox.vue';
import MdResultFilterToolbar from '@/components/custom/md-result-filter-toolbar.vue';
import MdResultSearch from '@/components/custom/md-result-search.vue';
import MdResultSummary from '@/components/custom/md-result-summary.vue';
import MdResultTable from '@/components/custom/md-result-table.vue';
import MdResultWorkspace from '@/components/custom/md-result-workspace.vue';
import MdSelectionActionBar from '@/components/custom/md-selection-action-bar.vue';
import MdDestructiveActionDialog from '@/components/custom/md-destructive-action-dialog.vue';
import MdApplicationClosePanel from '@/components/custom/md-application-close-panel.vue';
import MdConfirmDialog from '@/components/custom/md-confirm-dialog.vue';
import MdDialogContent from '@/components/custom/md-dialog-content.vue';
import MdDialogFooter from '@/components/custom/md-dialog-footer.vue';
import MdDialogHeader from '@/components/custom/md-dialog-header.vue';
import MdInlineNotice from '@/components/custom/md-inline-notice.vue';
import MdIcon from '@/components/icons/md-icon.vue';
import { Button } from '@/components/ui/button';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import MdCheckbox from '@/components/custom/md-checkbox.vue';
import { Dialog, DialogDescription, DialogTitle } from '@/components/ui/dialog';
import type {
  ApplicationUninstallBatchPlan,
  ApplicationUninstallBatchResult,
  ApplicationUninstallBatchSelection,
  ApplicationUninstallCandidate,
  ApplicationUninstallComponentSummary,
  ApplicationUninstallExecutionProgress,
  ApplicationUninstallScanResult,
} from '@/lib/models/applications/application';
import type {
  ApplicationCloseBatchResult,
  ApplicationCloseItem,
  ApplicationCloseMode,
} from '@/lib/models/applications/application-close';
import type { TraversalProgress } from '@/lib/models/presentation/progress';
import { ICON_NAMES } from '@/lib/models/presentation/ui';
import { ApplicationService } from '@/lib/services/applications/application-service';
import { ApplicationIconService } from '@/lib/services/applications/application-icon-service';
import { LoggerService } from '@/lib/services/diagnostics/logger-service';
import { ByteSizeService } from '@/lib/services/settings/byte-size-service';
import { OperatingSystemService } from '@/lib/services/settings/operating-system-service';
import { parseCommandError } from '@/lib/utils/diagnostics/error';

import {
  displayedApplications,
  applicationCatalogFilters,
  applicationCatalogSortAscending,
  applicationCatalogSortKey,
  applicationCanStartUninstall,
  applicationMatchesCatalogFilter,
  filterAndSortApplications,
  nextApplicationCatalogSort,
  type ApplicationCatalogFilter,
  type ApplicationCatalogSort,
  type ApplicationCatalogSortKey,
} from './application-uninstall-catalog';
import {
  applicationCloseRequestIds,
  applyApplicationCloseResult,
  beginApplicationCloseWorkflow,
  createApplicationUninstallCloseWorkflow,
  finishApplicationCloseWorkflow,
} from './application-uninstall-close-workflow';
import {
  selectedApplicationBytes,
  selectionIncludesUserData,
  setVisibleApplicationSelection,
  toggleApplicationComponent,
  toggleApplicationSelection,
} from './application-uninstall-selection';
import { applicationUninstallNotification, UNINSTALL_RESULT_TOAST_ID } from './application-uninstall-notification';
import MdApplicationUninstallRow from './components/md-application-uninstall-row.vue';
import MdApplicationUninstallPlanDialog from './components/md-application-uninstall-plan-dialog.vue';

const { t, locale } = useI18n({ useScope: 'global' });
const aiStore = useAiStore();
const props = defineProps<{
  catalog: ApplicationUninstallScanResult | null;
  scanning: boolean;
  cancelling: boolean;
  progress: TraversalProgress | null;
  executionProgress: ApplicationUninstallExecutionProgress | null;
  plan: ApplicationUninstallBatchPlan | null;
  preview: ApplicationUninstallBatchResult | null;
  lastResult: ApplicationUninstallBatchResult | null;
  preparing: boolean;
  executing: boolean;
  cancellingExecution: boolean;
  cancellationRevision: number;
  closingApplications: boolean;
  closeResult: ApplicationCloseBatchResult | null;
}>();
const emit = defineEmits<{
  scan: [];
  recordRemoved: [applicationId: string];
  cancelScan: [];
  prepare: [selections: ApplicationUninstallBatchSelection[]];
  cancelPlan: [];
  execute: [];
  cancelExecution: [];
  closeApplications: [applicationIds: string[], mode: ApplicationCloseMode];
  open: [path: string];
}>();

function toggleDetails(candidate: ApplicationUninstallCandidate): void {
  expandedId.value = expandedId.value === candidate.applicationId ? null : candidate.applicationId;
  const revision = props.catalog?.catalogRevision;
  if (expandedId.value && revision) {
    void ApplicationService.recordUninstallDetailsOpened(candidate.applicationId, revision).catch(() => {
      LoggerService.warn('application-uninstall', 'details_log_failed');
    });
  }
}

async function openWindowsInstalledApps(): Promise<void> {
  try {
    await ApplicationService.openWindowsInstalledApps();
  } catch {
    toast.error(t('applicationUninstall.openWindowsInstalledAppsFailed'));
  }
}

// Match startup management: opening the page starts with system entries hidden.
const showSystemItems = ref(false);
const query = ref('');
const filter = ref<ApplicationCatalogFilter>('all');
const sort = ref<ApplicationCatalogSort>('sizeDescending');
const expandedId = ref<string | null>(null);
const selectedIds = ref<string[]>([]);
const selectedComponentIds = ref<Record<string, string[]>>({});
const confirmOpen = ref(false);
const recordToRemove = ref<ApplicationUninstallCandidate | null>(null);
const removingRecord = ref(false);

async function removeRecord(): Promise<void> {
  const candidate = recordToRemove.value;
  if (!candidate || removingRecord.value) return;
  removingRecord.value = true;
  try {
    await ApplicationService.removeRecord(candidate.applicationId);
    recordToRemove.value = null;
    // The native command returns only after verifying removal. Reconcile that one row
    // without discarding the user's filter, selection or current scroll position.
    if (expandedId.value === candidate.applicationId) expandedId.value = null;
    emit('recordRemoved', candidate.applicationId);
    toast.success(t('applicationUninstall.removeRecordSuccess'));
  } catch (error) {
    const failure = parseCommandError(error);
    if (failure?.code === 'operationCancelled') {
      recordToRemove.value = null;
    } else if (failure?.details.mutationState === 'mayHaveChanged' || failure?.details.reason === 'itemChanged') {
      // The registration may already be gone. Refresh instead of inviting another stale removal.
      recordToRemove.value = null;
      toast.info(t('applicationUninstall.removeRecordRefreshing'));
      emit('scan');
    } else {
      toast.error(
        t(
          failure?.code === 'permissionDenied'
            ? 'applicationUninstall.removeRecordPermissionDenied'
            : 'applicationUninstall.removeRecordFailed'
        )
      );
    }
  } finally {
    removingRecord.value = false;
  }
}

const closeWorkflow = ref(createApplicationUninstallCloseWorkflow());
const cancellationConfirmOpen = ref(false);
const applicationList = ref<InstanceType<typeof MdResultTable> | null>(null);
const iconUrls = ref<ReadonlyMap<string, string>>(new Map());
const busy = computed(
  () => props.scanning || props.preparing || props.executing || props.closingApplications || removingRecord.value
);
const confirmationLoading = computed(() => props.preparing || (confirmOpen.value && !props.plan && !props.preview));
const candidates = computed(() => props.catalog?.candidates ?? []);
let explanationRequestVersion = 0;
async function explainApplication(candidate: ApplicationUninstallCandidate): Promise<void> {
  if (busy.value || !props.catalog) return;
  const version = ++explanationRequestVersion;
  const featureRevision = aiStore.featureRevision;
  const workspace = aiStore.workspaces.applicationUninstall;
  const selectionVersion = workspace.selectionVersion;
  const language = locale.value;
  const context = applicationUninstallAiContext(
    candidate,
    props.catalog,
    OperatingSystemService.isWindows() ? 'windows' : OperatingSystemService.isLinux() ? 'linux' : 'macos',
    selectedIds.value.includes(candidate.applicationId)
      ? (selectedComponentIds.value[candidate.applicationId] ?? [])
      : undefined
  );
  if (props.catalog.catalogRevision) {
    try {
      const identity = await ApplicationService.describeIdentity(
        candidate.applicationId,
        props.catalog.catalogRevision
      );
      if (identity.schemaVersion === 1 && identity.metadata && context.subject.module === 'applicationUninstall') {
        context.subject.identity = identity.metadata;
      }
    } catch (error) {
      LoggerService.warn('application-uninstall', 'identity_unavailable', {
        applicationId: candidate.applicationId,
        error,
      });
    }
  }
  // Identity preparation must obey the same cancellation boundaries as generation.
  if (
    version !== explanationRequestVersion ||
    busy.value ||
    featureRevision !== aiStore.featureRevision ||
    workspace !== aiStore.workspaces.applicationUninstall ||
    selectionVersion !== workspace.selectionVersion
  )
    return;
  void aiStore.show(context, language);
}
watch(
  () => [props.catalog, busy.value, selectedComponentIds.value],
  () => {
    ++explanationRequestVersion;
    aiStore.dismissModule('applicationUninstall');
  }
);
const windowsCatalog = OperatingSystemService.isWindows();
const hasUsageDate = !windowsCatalog && !OperatingSystemService.isLinux();
const catalogFilters = applicationCatalogFilters(windowsCatalog);
const dateSortOptions = computed(() => {
  const columns: [ApplicationCatalogSortKey, string][] = [
    ['name', 'applicationUninstall.applicationName'],
    ['size', 'applicationUninstall.applicationSize'],
    ['status', 'applicationUninstall.status'],
    ...(hasUsageDate ? [['lastUsed', 'applicationUninstall.lastUsed'] as [ApplicationCatalogSortKey, string]] : []),
    ['installed', windowsCatalog ? 'applicationUninstall.installedOrUpdated' : 'applicationUninstall.installed'],
  ];
  return columns.flatMap(([key, label]) => [
    { value: `${key}Descending`, label: `${t(label)} ↓` },
    { value: `${key}Ascending`, label: `${t(label)} ↑` },
  ]);
});
function selectCatalogSort(value: unknown): void {
  if (typeof value === 'string' && dateSortOptions.value.some(option => option.value === value)) {
    sort.value = value as ApplicationCatalogSort;
  }
}
const displayCandidates = computed(() => displayedApplications(candidates.value, showSystemItems.value));
const catalogBytes = computed(() =>
  displayCandidates.value.reduce((total, candidate) => total + candidate.totalBytes, 0)
);
const actionableCandidates = computed(() => displayCandidates.value.filter(applicationCanStartUninstall));
const filteredCandidates = computed(() =>
  filterAndSortApplications(
    displayCandidates.value,
    query.value,
    filter.value,
    sort.value,
    FormatService.timeZone(locale.value)
  )
);
const filteredReadyIds = computed(() =>
  filteredCandidates.value.filter(applicationCanStartUninstall).map(candidate => candidate.applicationId)
);
const selectedSet = computed(() => new Set(selectedIds.value));
const selectedCandidates = computed(() =>
  actionableCandidates.value.filter(candidate => selectedSet.value.has(candidate.applicationId))
);
const selection = computed(() => ({
  applicationIds: selectedIds.value,
  componentIds: selectedComponentIds.value,
}));
const selectedBytes = computed(() => selectedApplicationBytes(actionableCandidates.value, selection.value));
const includesUserData = computed(() => selectionIncludesUserData(actionableCandidates.value, selection.value));
const runningSelectedCandidates = computed(() =>
  selectedCandidates.value.filter(candidate => candidate.capability === 'applicationRunning')
);
const closeItems = computed<ApplicationCloseItem[]>(() =>
  runningSelectedCandidates.value.map(candidate => ({
    id: candidate.applicationId,
    iconPath: candidate.iconPath ?? undefined,
    name: candidate.name,
    processes: candidate.runningProcesses,
  }))
);
const remainingCloseItems = computed(() => {
  const remaining = new Set(closeWorkflow.value.remainingApplicationIds);
  return closeItems.value.filter(item => remaining.has(item.id));
});
const allFilteredSelected = computed(
  () =>
    filteredReadyIds.value.length > 0 &&
    filteredReadyIds.value.every(applicationId => selectedSet.value.has(applicationId))
);
const someFilteredSelected = computed(() =>
  filteredReadyIds.value.some(applicationId => selectedSet.value.has(applicationId))
);
const filterCounts = computed(() => ({
  all: displayCandidates.value.length,
  ready: displayCandidates.value.filter(candidate => applicationMatchesCatalogFilter(candidate, 'ready')).length,
  requiresElevation: displayCandidates.value.filter(candidate =>
    applicationMatchesCatalogFilter(candidate, 'requiresElevation')
  ).length,
  running: displayCandidates.value.filter(candidate => applicationMatchesCatalogFilter(candidate, 'running')).length,
  unavailable: displayCandidates.value.filter(candidate => applicationMatchesCatalogFilter(candidate, 'unavailable'))
    .length,
}));
const filterOptions = computed(() =>
  catalogFilters.map(value => ({
    value,
    label: t(`applicationUninstall.${value}`),
    count: filterCounts.value[value],
  }))
);

function updateCatalogFilter(value: string) {
  const nextFilter = catalogFilters.find(option => option === value);
  if (nextFilter) filter.value = nextFilter;
}
const scanStage = computed(() => props.progress?.currentStage ?? 'discoveringApplications');
const scanStageText = computed(() => {
  if (scanStage.value === 'checkingProcesses') return t('applicationUninstall.checkingProcesses');
  if (scanStage.value === 'validatingApplications') return t('applicationUninstall.validatingApplications');
  if (scanStage.value === 'inspectingApplications') return t('applicationUninstall.inspectingApplication');
  return t('applicationUninstall.readingApplicationCatalog');
});
const scanPathLabel = computed(() =>
  scanStage.value === 'inspectingApplications'
    ? t('applicationUninstall.currentApplication')
    : scanStage.value === 'discoveringApplications' || scanStage.value === 'validatingApplications'
      ? t('applicationUninstall.currentApplicationSource')
      : t('applicationUninstall.currentScanStage')
);
let iconRequestVersion = 0;

watch(
  () => filteredCandidates.value.map(candidate => candidate.iconPath).filter((path): path is string => Boolean(path)),
  paths => {
    const requestVersion = ++iconRequestVersion;
    if (!paths.length) {
      if (!candidates.value.length) iconUrls.value = new Map();
      return;
    }

    void ApplicationIconService.resolveIncrementally(paths, icons => {
      if (requestVersion !== iconRequestVersion) return;
      iconUrls.value = new Map([...iconUrls.value, ...icons]);
    });
  },
  { immediate: true }
);

onBeforeUnmount(() => {
  iconRequestVersion += 1;
  explanationRequestVersion += 1;
});

function handleApplicationIconError(iconPath: string | null) {
  if (!iconPath) return;
  const icons = new Map(iconUrls.value);
  icons.delete(iconPath);
  iconUrls.value = icons;
}

watch([query, filter, sort, showSystemItems], async () => {
  // A filter or sort can shrink a long result. Reset the domain-owned scroll
  // position after rendering so an old offset never leaves a blank list.
  await nextTick();
  applicationList.value?.scrollTo({ top: 0 });
});

watch(
  () => actionableCandidates.value.map(candidate => candidate.applicationId),
  readyIds => {
    const readySet = new Set(readyIds);
    const deselectedCount = selectedIds.value.filter(id => !readySet.has(id)).length;
    if (deselectedCount > 0) {
      LoggerService.info(
        'application-uninstall',
        `selection_pruned deselected_count=${deselectedCount} show_system_items=${showSystemItems.value}`
      );
    }
    selectedIds.value = selectedIds.value.filter(applicationId => readySet.has(applicationId));
    selectedComponentIds.value = Object.fromEntries(
      Object.entries(selectedComponentIds.value).filter(([applicationId]) => readySet.has(applicationId))
    );
  }
);

// Native logging intentionally omits arbitrary context objects. Embed only these finite flags
// and counts in the persisted event, never search text, package identities, names or paths.
watch(
  [() => props.catalog, showSystemItems],
  () => {
    if (!props.catalog || !windowsCatalog) return;
    LoggerService.info(
      'application-uninstall',
      `system_filter_applied scan_time_ms=${props.catalog.scannedAtMs} schema_version=${props.catalog.schemaVersion} show_system_items=${showSystemItems.value} candidate_count=${candidates.value.length} visible_count=${displayCandidates.value.length} hidden_system_count=${candidates.value.length - displayCandidates.value.length}`
    );
  },
  { immediate: true }
);

watch(
  () => props.plan?.batchId,
  batchId => {
    confirmOpen.value = Boolean(batchId);
  }
);

watch(
  () => props.preparing,
  (preparing, wasPreparing) => {
    if (wasPreparing && !preparing && !props.plan) confirmOpen.value = false;
  }
);

watch(
  () => props.cancellationRevision,
  (revision, previousRevision) => {
    if (!revision || revision === previousRevision) return;
    toast.info(t('applicationUninstall.cancelledTitle'), {
      id: UNINSTALL_RESULT_TOAST_ID,
    });
  }
);

watch(
  () => props.lastResult,
  result => {
    if (!result) return;

    // Clear the completed selection immediately so the action bar cannot
    // submit the same batch again while the result toast is being published.
    selectedIds.value = [];
    selectedComponentIds.value = {};
    expandedId.value = null;

    const notification = applicationUninstallNotification(result, t, {
      integer: FormatService.integer,
      bytes: value => ByteSizeService.bytes(value),
    });
    toast[notification.tone](notification.title, notification.options);
  }
);

function toggleSelection(candidate: ApplicationUninstallCandidate) {
  if (busy.value) return;
  const next = toggleApplicationSelection(selection.value, candidate);
  selectedIds.value = next.applicationIds;
  selectedComponentIds.value = next.componentIds;
}

function toggleComponent(candidate: ApplicationUninstallCandidate, component: ApplicationUninstallComponentSummary) {
  if (busy.value) return;
  const next = toggleApplicationComponent(selection.value, candidate, component);
  selectedIds.value = next.applicationIds;
  selectedComponentIds.value = next.componentIds;
}

function toggleFilteredSelection(checked: boolean) {
  if (busy.value || !filteredReadyIds.value.length) return;
  const next = setVisibleApplicationSelection(
    selection.value,
    filteredCandidates.value.filter(applicationCanStartUninstall),
    checked
  );
  selectedIds.value = next.applicationIds;
  selectedComponentIds.value = next.componentIds;
}

function clearSelection() {
  if (busy.value) return;
  selectedIds.value = [];
  selectedComponentIds.value = {};
}

function prepareSelection() {
  if (busy.value || !selectedIds.value.length) return;
  if (runningSelectedCandidates.value.length) {
    closeWorkflow.value = beginApplicationCloseWorkflow(
      runningSelectedCandidates.value.map(candidate => candidate.applicationId)
    );
    return;
  }
  prepareCurrentSelection();
}

function prepareCurrentSelection() {
  if (!selectedIds.value.length) return;
  confirmOpen.value = true;
  emit(
    'prepare',
    selectedIds.value.map(applicationId => ({
      applicationId,
      componentIds: selectedComponentIds.value[applicationId] ?? [],
    }))
  );
}

function prepareApplication(candidate: ApplicationUninstallCandidate) {
  if (busy.value || !applicationCanStartUninstall(candidate)) return;
  /*
   * Row uninstall is an explicit single-application shortcut, so it reuses
   * the existing selection policy from an empty selection. Required/default
   * components stay consistent without carrying earlier choices into review.
   */
  const next = toggleApplicationSelection({ applicationIds: [], componentIds: {} }, candidate);
  if (!next.applicationIds.length) return;
  selectedIds.value = next.applicationIds;
  selectedComponentIds.value = next.componentIds;
  prepareSelection();
}

function requestApplicationClose(mode: ApplicationCloseMode) {
  if (props.closingApplications) return;
  const applicationIds = applicationCloseRequestIds(closeWorkflow.value, mode);
  if (!applicationIds.length) {
    finishApplicationClose([]);
    return;
  }
  emit('closeApplications', applicationIds, mode);
}

function updateCloseDialog(open: boolean) {
  // A close request owns the process snapshot until it completes. Keeping the
  // dialog mounted prevents a late result from being applied to a new choice.
  if (!open && props.closingApplications) return;
  closeWorkflow.value.open = open;
  if (open) return;
  closeWorkflow.value = createApplicationUninstallCloseWorkflow();
}

function finishApplicationClose(skippedApplicationIds: string[]) {
  const nextSelection = finishApplicationCloseWorkflow(selection.value, closeWorkflow.value, skippedApplicationIds);
  selectedIds.value = nextSelection.applicationIds;
  selectedComponentIds.value = nextSelection.componentIds;
  closeWorkflow.value = createApplicationUninstallCloseWorkflow();
  if (selectedIds.value.length) prepareCurrentSelection();
}

watch(
  () => props.closeResult,
  result => {
    if (!closeWorkflow.value.open || !result) return;
    const transition = applyApplicationCloseResult(closeWorkflow.value, result);
    if (!transition.completed) {
      closeWorkflow.value = transition.workflow;
      return;
    }
    finishApplicationClose([]);
  }
);

function changeSort(key: ApplicationCatalogSortKey) {
  sort.value = nextApplicationCatalogSort(sort.value, key);
}

function sortIcon(key: ApplicationCatalogSortKey) {
  if (applicationCatalogSortKey(sort.value) !== key) return ICON_NAMES.arrowUpDown;
  return applicationCatalogSortAscending(sort.value) ? ICON_NAMES.arrowUp : ICON_NAMES.arrowDown;
}

function updateConfirmation(open: boolean) {
  if (props.executing) return;
  confirmOpen.value = open;
  if (!open) {
    emit('cancelPlan');
  }
}

function requestCancelExecution() {
  if (!props.executing || props.cancellingExecution) return;
  cancellationConfirmOpen.value = true;
}

function confirmCancelExecution() {
  if (!props.executing || props.cancellingExecution) return;
  cancellationConfirmOpen.value = false;
  emit('cancelExecution');
}
</script>

<template>
  <MdPageShell
    class="@container/application-uninstall"
    content-mode="workspace"
    :title="t('applicationUninstall.title')"
  >
    <template #overlay><MdAiWorkspace module="applicationUninstall" /></template>
    <template v-if="catalog" #actions>
      <Button variant="outline" type="button" :disabled="busy" @click="emit('scan')">
        <MdIcon :class="{ 'icon-spin': scanning }" :name="ICON_NAMES.refresh" :size="17" />
        {{ scanning ? t('loading.currentStage') : t('applicationUninstall.rescan') }}
      </Button>
    </template>

    <MdOperationWorkspace v-if="scanning">
      <MdOperationProgress
        :icon-name="ICON_NAMES.uninstall"
        :title="cancelling ? t('loading.cancelling') : t('applicationUninstall.scanning')"
        :progress="progress"
        :path-label="scanPathLabel"
        :preparing-text="scanStageText"
        :hint="t('applicationUninstall.scanHint')"
        :items-label="t('applicationUninstall.checkedApplications')"
        :bytes-label="t('applicationUninstall.scannedApplicationData')"
        :cancelable="true"
        :cancel-disabled="cancelling"
        @cancel="emit('cancelScan')"
      />
    </MdOperationWorkspace>

    <MdResultWorkspace v-else>
      <template v-if="catalog?.catalogActionable" #summary>
        <MdResultSummary
          :title="
            t(
              'applicationUninstall.summary',
              { count: FormatService.integer(displayCandidates.length) },
              displayCandidates.length
            )
          "
          :metric-label="t('applicationUninstall.summarySpace')"
          :metric-value="ByteSizeService.bytes(catalogBytes)"
        >
          <template #actions>
            <label
              v-if="windowsCatalog"
              class="flex items-center gap-2 whitespace-nowrap text-sm text-muted-foreground"
            >
              <MdCheckbox v-model="showSystemItems" :disabled="busy || confirmationLoading || confirmOpen" />
              {{ t('applicationUninstall.showSystemItems') }}
            </label>
          </template>
        </MdResultSummary>
      </template>

      <template v-if="catalog?.catalogActionable" #header>
        <MdResultFilterToolbar>
          <MdCategoryFilter
            :model-value="filter"
            :options="filterOptions"
            :accessibility-label="t('applicationUninstall.filterLabel')"
            :disabled="busy"
            @update:model-value="updateCatalogFilter"
          />
          <template #aside>
            <div class="compact-date-sort">
              <Select :model-value="sort" :disabled="busy" @update:model-value="selectCatalogSort">
                <SelectTrigger size="sm" :aria-label="t('applicationUninstall.dateSortLabel')">
                  <SelectValue>{{ dateSortOptions.find(option => option.value === sort)?.label }}</SelectValue>
                </SelectTrigger>
                <SelectContent>
                  <SelectItem v-for="option in dateSortOptions" :key="option.value" :value="option.value">{{
                    option.label
                  }}</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <MdResultSearch v-model="query" :placeholder="t('applicationUninstall.searchPlaceholder')" />
          </template>
        </MdResultFilterToolbar>
      </template>

      <template v-if="catalog">
        <MdInlineNotice
          v-if="preview?.failedItemCount && !plan"
          class="catalog-notice"
          :icon-name="ICON_NAMES.info"
          :title="t('applicationUninstall.preflightFailed')"
          tone="warning"
          role="alert"
        >
          {{ t('applicationUninstall.batchPreflightFailedSummary') }}
        </MdInlineNotice>

        <MdEmptyState
          v-if="!catalog.catalogActionable"
          :icon-name="ICON_NAMES.info"
          :title="t('applicationUninstall.incompleteTitle')"
          :description="t('applicationUninstall.incompleteDescription')"
        />
        <template v-else>
          <MdInlineNotice
            v-if="!catalog.executionSupported"
            class="catalog-notice"
            :icon-name="ICON_NAMES.info"
            :title="t('applicationUninstall.viewOnlyTitle')"
          >
            {{ t('applicationUninstall.viewOnlyDescription') }}
          </MdInlineNotice>

          <section class="catalog">
            <MdEmptyState
              v-if="!filteredCandidates.length"
              class="catalog-empty"
              compact
              :icon-name="ICON_NAMES.search"
              :title="t('applicationUninstall.emptyTitle')"
              :description="t('applicationUninstall.emptyDescription')"
            />

            <MdResultTable v-else ref="applicationList" class="application-list">
              <template #header>
                <div class="application-list-header" :class="{ 'has-usage-date': hasUsageDate }">
                  <label>
                    <MdResultCheckbox
                      :checked="allFilteredSelected"
                      :indeterminate="someFilteredSelected && !allFilteredSelected"
                      :disabled="busy || !filteredReadyIds.length"
                      :aria-label="t('applicationUninstall.selectVisible')"
                      @update:checked="toggleFilteredSelection"
                    />
                  </label>
                  <button
                    class="name-header md-result-sort"
                    type="button"
                    :data-active="applicationCatalogSortKey(sort) === 'name'"
                    :aria-label="t('applicationUninstall.sortByName')"
                    @click="changeSort('name')"
                  >
                    {{ t('applicationUninstall.applicationName') }}
                    <MdIcon :name="sortIcon('name')" :size="14" />
                  </button>
                  <button
                    class="status-header md-result-sort"
                    type="button"
                    :data-active="applicationCatalogSortKey(sort) === 'status'"
                    :aria-label="t('applicationUninstall.sortByStatus')"
                    @click="changeSort('status')"
                  >
                    {{ t('applicationUninstall.status') }}
                    <MdIcon :name="sortIcon('status')" :size="14" />
                  </button>
                  <button
                    class="size-header md-result-sort"
                    type="button"
                    :data-active="applicationCatalogSortKey(sort) === 'size'"
                    :aria-label="t('applicationUninstall.sortBySize')"
                    @click="changeSort('size')"
                  >
                    {{ t('applicationUninstall.applicationSize') }}
                    <MdIcon :name="sortIcon('size')" :size="14" />
                  </button>
                  <button
                    v-if="hasUsageDate"
                    class="application-date-header md-result-sort"
                    type="button"
                    :data-active="applicationCatalogSortKey(sort) === 'lastUsed'"
                    :aria-label="t('applicationUninstall.sortByLastUsed')"
                    @click="changeSort('lastUsed')"
                  >
                    {{ t('applicationUninstall.lastUsed') }}
                    <MdIcon :name="sortIcon('lastUsed')" :size="14" />
                  </button>
                  <button
                    class="application-date-header md-result-sort"
                    type="button"
                    :data-active="applicationCatalogSortKey(sort) === 'installed'"
                    :aria-label="t('applicationUninstall.sortByInstalled')"
                    @click="changeSort('installed')"
                  >
                    {{
                      t(windowsCatalog ? 'applicationUninstall.installedOrUpdated' : 'applicationUninstall.installed')
                    }}
                    <MdIcon :name="sortIcon('installed')" :size="14" />
                  </button>
                  <span />
                </div>
              </template>

              <MdApplicationUninstallRow
                v-for="candidate in filteredCandidates"
                :key="candidate.applicationId"
                :candidate="candidate"
                :icon-src="candidate.iconPath ? iconUrls.get(candidate.iconPath) : undefined"
                :selected="selectedSet.has(candidate.applicationId)"
                :selected-component-ids="selectedComponentIds[candidate.applicationId] ?? []"
                :expanded="expandedId === candidate.applicationId"
                :busy="busy"
                :uninstall-enabled="catalog.executionSupported"
                @toggle-selection="toggleSelection(candidate)"
                @toggle-component="toggleComponent(candidate, $event)"
                @toggle-expanded="toggleDetails(candidate)"
                @open="emit('open', $event)"
                @open-windows-settings="openWindowsInstalledApps"
                @remove-record="recordToRemove = candidate"
                @uninstall="prepareApplication(candidate)"
                @explain="explainApplication(candidate)"
                @icon-error="handleApplicationIconError(candidate.iconPath)"
              />
            </MdResultTable>
          </section>
        </template>
      </template>

      <MdEmptyState
        v-else
        :icon-name="ICON_NAMES.uninstall"
        :title="t('applicationUninstall.initialTitle')"
        :description="t('applicationUninstall.initialDescription')"
      >
        <Button size="lg" type="button" :disabled="busy" @click="emit('scan')">
          <MdIcon :name="ICON_NAMES.scan" :size="17" />
          {{ t('applicationUninstall.startScan') }}
        </Button>
      </MdEmptyState>
    </MdResultWorkspace>

    <template v-if="!scanning && catalog?.catalogActionable && catalog.executionSupported" #footer>
      <MdSelectionActionBar
        :selected-label="t('applicationUninstall.selectedApplicationsLabel')"
        :selected-value="FormatService.integer(selectedCandidates.length)"
        :space-label="t('applicationUninstall.selectedSizeLabel')"
        :space-value="ByteSizeService.bytes(selectedBytes)"
        :clear-label="selectedCandidates.length ? t('applicationUninstall.clearSelection') : undefined"
        :action-label="t('applicationUninstall.uninstallSelected')"
        :disabled="!selectedCandidates.length"
        :busy="busy"
        @clear="clearSelection"
        @action="prepareSelection"
      >
        <template #action-icon>
          <span v-if="preparing || executing" class="button-spinner" />
          <MdIcon v-else :name="ICON_NAMES.uninstall" :size="17" />
        </template>
      </MdSelectionActionBar>
    </template>

    <MdDestructiveActionDialog
      :open="recordToRemove !== null"
      :title="t('applicationUninstall.removeRecordTitle')"
      :summary-label="recordToRemove?.name ?? ''"
      :description="t('applicationUninstall.removeRecordDescription')"
      :cancel-label="t('common.cancel')"
      :confirm-label="t('applicationUninstall.removeRecord')"
      :busy="removingRecord"
      @update:open="
        open => {
          if (!open) recordToRemove = null;
        }
      "
      @confirm="removeRecord"
    />
    <MdApplicationUninstallPlanDialog
      :open="confirmOpen"
      :plan="plan"
      :preview="preview"
      :candidates="candidates"
      :selected-candidates="selectedCandidates"
      :selected-bytes="selectedBytes"
      :includes-user-data="includesUserData"
      :preparing="preparing"
      :executing="executing"
      :cancelling-execution="cancellingExecution"
      :confirmation-loading="confirmationLoading"
      :execution-progress="executionProgress"
      @update:open="updateConfirmation"
      @execute="emit('execute')"
      @cancel-execution="requestCancelExecution"
    />

    <Dialog :open="closeWorkflow.open" @update:open="updateCloseDialog">
      <MdDialogContent class="flex min-h-0 flex-col" size="large" :show-close="!closingApplications">
        <MdDialogHeader>
          <DialogTitle>{{ t('applicationUninstall.closeBeforeUninstallTitle') }}</DialogTitle>
          <DialogDescription>
            {{
              closeWorkflow.phase === 'selection'
                ? t('applicationUninstall.closeBeforeUninstallDescription')
                : t('applicationClose.normalCloseFailed')
            }}
          </DialogDescription>
        </MdDialogHeader>

        <div class="min-h-0 overflow-auto px-6 pb-4">
          <p v-if="closeWorkflow.phase === 'force'" class="uninstall-force-close-warning">
            {{ t('applicationClose.forceWarning') }}
          </p>
          <MdApplicationClosePanel
            v-if="closeWorkflow.phase === 'selection'"
            v-model:selected-ids="closeWorkflow.selectedApplicationIds"
            :items="closeItems"
            :disabled="closingApplications"
          />
          <MdApplicationClosePanel v-else :items="remainingCloseItems" :selectable="false" />
        </div>

        <MdDialogFooter>
          <Button
            v-if="closeWorkflow.phase === 'selection'"
            variant="outline"
            type="button"
            :disabled="closingApplications"
            @click="updateCloseDialog(false)"
          >
            {{ t('common.cancel') }}
          </Button>
          <Button
            v-else
            variant="outline"
            type="button"
            :disabled="closingApplications"
            @click="finishApplicationClose(closeWorkflow.remainingApplicationIds)"
          >
            {{ t('applicationClose.skipAndContinue') }}
          </Button>
          <Button
            :variant="closeWorkflow.phase === 'force' ? 'destructive' : 'default'"
            type="button"
            :disabled="closingApplications"
            @click="requestApplicationClose(closeWorkflow.phase === 'force' ? 'force' : 'graceful')"
          >
            {{
              closingApplications
                ? t('applicationClose.closing')
                : closeWorkflow.phase === 'force'
                  ? t('applicationClose.forceAndContinue')
                  : closeWorkflow.selectedApplicationIds.length
                    ? t(
                        'applicationClose.closeSelectedAndContinue',
                        { count: FormatService.integer(closeWorkflow.selectedApplicationIds.length) },
                        closeWorkflow.selectedApplicationIds.length
                      )
                    : t('applicationClose.skipAndContinue')
            }}
          </Button>
        </MdDialogFooter>
      </MdDialogContent>
    </Dialog>

    <MdConfirmDialog
      v-model:open="cancellationConfirmOpen"
      :title="t('applicationUninstall.cancelExecutionConfirmTitle')"
      :description="t('applicationUninstall.cancelExecutionConfirmDescription')"
      :cancel-label="t('common.cancel')"
      :confirm-label="t('applicationUninstall.stopExecutionAction')"
      confirm-variant="destructive"
      @confirm="confirmCancelExecution"
    />
  </MdPageShell>
</template>

<style scoped src="./application-uninstall.css"></style>
