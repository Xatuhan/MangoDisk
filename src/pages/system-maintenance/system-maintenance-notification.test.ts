// @vitest-environment happy-dom
import { mount } from '@vue/test-utils';
import { computed, h, KeepAlive, nextTick } from 'vue';
import { createPinia, setActivePinia } from 'pinia';
import { createI18n } from 'vue-i18n';
import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';
import { toast } from 'vue-sonner';
import en from '@/locales/en-US.json';
import type {
  SystemMaintenanceExecutionItemResult,
  SystemMaintenanceJob,
} from '@/lib/models/system-maintenance/system-maintenance';
import { useSystemMaintenanceStore } from '@/stores/system-maintenance-store';
import { useAppStore } from '@/stores/app-store';
import { PAGE_IDS } from '@/lib/models/shell/application-shell';
import Page from './index.vue';

vi.mock('vue-sonner', () => ({ toast: { success: vi.fn(), info: vi.fn(), warning: vi.fn() } }));
vi.mock('@/stores/ai-store', () => ({ useAiStore: () => ({ dismissModule: vi.fn() }) }));

const taskId = 'macos.maintenance.finder-service';
const completed: SystemMaintenanceExecutionItemResult = {
  taskId,
  status: 'completed',
  mutationState: 'changed',
  verified: true,
  requiresRestart: false,
  failureReason: null,
};

function render() {
  const store = useSystemMaintenanceStore();
  vi.spyOn(store, 'initialize').mockResolvedValue();
  store.catalog = {
    schemaVersion: 1,
    scanId: 'scan',
    platform: 'macos',
    scannedAtMs: 0,
    elapsedMs: 0,
    items: [
      {
        taskId,
        category: 'searchAndInterface',
        riskLevel: 'standard',
        status: 'recommended',
        requiresElevation: false,
        requiresRestart: false,
        estimatedDurationSeconds: 1,
        diagnostic: null,
      },
    ],
    summary: { itemCount: 1, recommendedCount: 1, availableCount: 0, healthyCount: 0, unavailableCount: 0 },
  };
  const appStore = useAppStore();
  appStore.currentPage = PAGE_IDS.systemMaintenance;
  const active = computed({
    get: () => appStore.currentPage === PAGE_IDS.systemMaintenance,
    set: value => appStore.navigate(value ? PAGE_IDS.systemMaintenance : PAGE_IDS.cleanup),
  });
  const wrapper = mount(
    { setup: () => () => h(KeepAlive, () => (active.value ? h(Page) : h('div'))) },
    {
      shallow: true,
      global: {
        plugins: [createI18n({ legacy: false, locale: 'en-US', messages: { 'en-US': en } })],
        renderStubDefaultSlot: true,
        stubs: {
          index: false,
          KeepAlive: false,
          MdCatalogListItem: { template: '<article><slot name="description" /><slot name="actions" /></article>' },
        },
      },
    }
  );
  function finish(result: SystemMaintenanceExecutionItemResult) {
    const job: SystemMaintenanceJob = {
      executionId: 'execution',
      scanId: 'scan',
      taskId,
      revision: 1,
      status: 'finished',
      cancelable: false,
      queuedAtMs: 0,
      startedAtMs: 0,
      finishedAtMs: 1,
      progress: null,
      result,
    };
    store.executions = { execution: job };
    store.lastResult = result;
  }
  return { wrapper, active, finish, store };
}

beforeEach(() => {
  setActivePinia(createPinia());
  vi.clearAllMocks();
});
afterEach(() => vi.restoreAllMocks());

describe('maintenance completion notifications', () => {
  it('shows a verified success in the row without another toast', async () => {
    const { wrapper, finish } = render();
    finish({ ...completed });
    await nextTick();
    expect(wrapper.text()).toContain(en.systemMaintenance.completedOne);
    expect(toast.success).not.toHaveBeenCalled();
    wrapper.unmount();
  });
  it('retains success feedback after navigating away', async () => {
    const { wrapper, active, finish } = render();
    active.value = false;
    await nextTick();
    finish({ ...completed });
    await nextTick();
    expect(toast.success).toHaveBeenCalledOnce();
    wrapper.unmount();
  });
  it('retains completion feedback when navigation and completion occur in the same render cycle', async () => {
    const { wrapper, active, finish } = render();
    active.value = false;
    finish({ ...completed });
    await nextTick();
    expect(toast.success).toHaveBeenCalledOnce();
    wrapper.unmount();
  });
  it.each([
    { requiresRestart: true },
    { verified: false },
    { mutationState: 'mayHaveChanged' as const },
    { status: 'started' as const },
  ])('retains important or uncertain completion feedback: %j', async overrides => {
    const { wrapper, finish } = render();
    finish({ ...completed, ...overrides });
    await nextTick();
    expect(toast.success).toHaveBeenCalledOnce();
    wrapper.unmount();
  });
  it.each(['permissionDenied', 'verificationFailed', 'userCancelled'] as const)(
    'retains %s warnings after possible changes',
    async failureReason => {
      const { wrapper, finish } = render();
      finish({ ...completed, status: 'failed', mutationState: 'mayHaveChanged', verified: false, failureReason });
      await nextTick();
      expect(toast.warning).toHaveBeenCalledOnce();
      expect(toast.success).not.toHaveBeenCalled();
      wrapper.unmount();
    }
  );
});
