// @vitest-environment happy-dom
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia } from 'pinia';
import { expect, it, vi } from 'vitest';
import { i18n } from '@/i18n';
import type { PresentedScanRuleResult } from '@/lib/models/cleanup/cleanup';
import { ByteSizeService } from '@/lib/services/settings/byte-size-service';
import MdResultItemContent from '@/components/custom/md-result-item-content.vue';
import MdResultCheckbox from '@/components/custom/md-result-checkbox.vue';
import MdCleanupRuleGroups from './md-cleanup-rule-groups.vue';

vi.mock('@tauri-apps/plugin-os', () => ({ platform: () => 'windows' }));

const rule: PresentedScanRuleResult = {
  ruleId: 'project.rust-build-artifacts',
  category: 'project',
  group: 'project',
  risk: 'recoverable',
  defaultSelected: false,
  recommendedSelected: false,
  bytes: 200,
  fileCount: 1,
  available: true,
  selectable: true,
  status: 'found',
  runningProcesses: ['Codex'],
  requiresAppClose: false,
  sources: [{ path: '/fixture/target', bytes: 200, fileCount: 1, modifiedAtMs: null, blockReason: 'requiresClose' }],
  sourceCount: 1,
  sourcesTruncated: false,
  scanElapsedMs: 1,
  name: 'Rust artifacts',
  description: 'Build output',
  impact: 'Source files remain',
  categoryLabel: 'Projects',
};

it.each(['requiresClose', 'limited'] as const)(
  'shows blocked ComfyUI models without enabling cleanup for %s',
  async status => {
    const blocked: PresentedScanRuleResult = {
      ...rule,
      ruleId: 'special.ai-model-comfyui',
      name: 'ComfyUI local models',
      category: 'ai',
      group: 'ai',
      selectable: false,
      status,
      runningProcesses: status === 'requiresClose' ? ['python3'] : [],
      sources: [
        { path: '/fixture/model.safetensors', bytes: 200, fileCount: 1, modifiedAtMs: null, blockReason: null },
      ],
    };
    const wrapper = mount(MdCleanupRuleGroups, {
      props: {
        busy: false,
        leftovers: null,
        rules: [blocked],
        selectedLeftoverIds: [],
        selectedRuleIds: [],
        sourceSelections: [],
        privilegedScanRuleId: null,
      },
      global: {
        plugins: [createPinia(), i18n],
        stubs: {
          MdResultTable: { methods: { scrollTo() {} }, template: '<div><slot name="header" /><slot /></div>' },
          MdAiAction: true,
        },
      },
    });
    await flushPromises();
    expect(wrapper.text()).toContain(blocked.name);
    expect(wrapper.findComponent(MdResultItemContent).props('valueDetail')).toBe(
      i18n.global.t(status === 'limited' ? 'cleanup.sourceAccessLimited' : 'cleanup.sourceRequiresClose')
    );
    for (const checkbox of wrapper.findAllComponents(MdResultCheckbox)) {
      expect(checkbox.props('disabled')).toBe(true);
    }
    expect(wrapper.emitted('selectAll')).toBeUndefined();
    wrapper.unmount();
  }
);

it('shares one category restart action and reserves admin check for previous installations', async () => {
  const wrapper = mount(MdCleanupRuleGroups, {
    props: {
      busy: false,
      leftovers: null,
      rules: [
        'special.windows-system-logs',
        'special.windows-update-cleanup',
        'special.windows-previous-installations',
      ].map(
        ruleId =>
          ({
            ...rule,
            ruleId,
            category: 'system',
            group: 'system',
            bytes: ruleId === 'special.windows-system-logs' ? 12 * 1024 * 1024 : 0,
            selectable: false,
            status: 'requiresElevation',
            runningProcesses: [],
            sources: [],
            sourceCount: 0,
          }) as PresentedScanRuleResult
      ),
      selectedLeftoverIds: [],
      selectedRuleIds: [],
      sourceSelections: [],
      privilegedScanRuleId: null,
    },
    global: {
      plugins: [createPinia(), i18n],
      stubs: {
        MdResultTable: { methods: { scrollTo() {} }, template: '<div><slot name="header" /><slot /></div>' },
        MdAiAction: true,
      },
    },
  });
  await flushPromises();
  const summaries = wrapper.findAllComponents(MdResultItemContent);
  expect(summaries.map(summary => summary.props('description'))).toEqual([undefined, undefined, undefined]);
  expect(summaries.map(summary => summary.props('value'))).toEqual([
    i18n.global.t('cleanup.privilegedScan.adminRequired'),
    i18n.global.t('cleanup.privilegedScan.adminRequired'),
    i18n.global.t('cleanup.privilegedScan.required'),
  ]);
  expect(summaries.map(summary => summary.props('valueTone'))).toEqual(['default', 'default', 'warning']);
  expect(summaries.map(summary => summary.props('valueDetail'))).toEqual([
    undefined,
    undefined,
    i18n.global.t('cleanup.privilegedScan.sizePending'),
  ]);
  const restartButtons = wrapper.findAll('.administrator-restart-button');
  expect(restartButtons).toHaveLength(1);
  expect(wrapper.find('.administrator-restart-notice').text()).toContain(
    i18n.global.t('cleanup.privilegedScan.restartNotice')
  );
  expect(wrapper.findAll('.rule-summary .administrator-restart-button')).toHaveLength(0);
  await restartButtons[0]!.trigger('click');
  expect(wrapper.emitted('restartAsAdministrator')).toEqual([[]]);
  for (const blockingProps of [
    { busy: true },
    { administratorRestartDisabled: true },
    { administratorRestartBusy: true },
    { privilegedScanRuleId: 'special.windows-previous-installations' },
  ]) {
    await wrapper.setProps({
      busy: false,
      administratorRestartDisabled: false,
      administratorRestartBusy: false,
      privilegedScanRuleId: null,
      ...blockingProps,
    });
    expect(restartButtons.every(button => button.attributes('disabled') !== undefined)).toBe(true);
    await restartButtons[0]!.trigger('click');
    expect(wrapper.emitted('restartAsAdministrator')).toHaveLength(1);
  }
  await wrapper.setProps({
    busy: false,
    administratorRestartDisabled: false,
    administratorRestartBusy: false,
    privilegedScanRuleId: null,
  });
  const buttons = wrapper.findAll('.privileged-scan-button');
  expect(buttons).toHaveLength(1);
  await buttons[0]!.trigger('click');
  expect(wrapper.emitted('privilegedScan')).toEqual([[]]);
  for (const checkbox of wrapper.findAllComponents(MdResultCheckbox)) {
    expect(checkbox.props('disabled')).toBe(true);
  }
  await wrapper.setProps({
    rules: wrapper.props('rules').filter(rule => rule.ruleId === 'special.windows-previous-installations'),
  });
  expect(wrapper.find('.administrator-restart-notice').exists()).toBe(false);
  expect(wrapper.find('.privileged-scan-button').exists()).toBe(true);
  wrapper.unmount();
});

it('keeps a single rule collapsed and allows selecting its close-required source after expansion', async () => {
  const wrapper = mount(MdCleanupRuleGroups, {
    props: {
      busy: false,
      leftovers: null,
      rules: [rule],
      selectedLeftoverIds: [],
      selectedRuleIds: [],
      sourceSelections: [],
      privilegedScanRuleId: null,
    },
    global: {
      plugins: [createPinia(), i18n],
      stubs: {
        MdResultTable: { methods: { scrollTo() {} }, template: '<div><slot name="header" /><slot /></div>' },
        MdAiAction: true,
        MdIconAction: true,
      },
    },
  });
  await flushPromises();
  expect(wrapper.find('.result-detail-metric small').exists()).toBe(false);
  expect(wrapper.find('.rule-details').exists()).toBe(false);
  const summary = wrapper.findComponent(MdResultItemContent);
  expect(summary.props('expanded')).toBe(false);
  summary.vm.$emit('toggle');
  await flushPromises();
  const source = wrapper.get('.source-row').findComponent(MdResultCheckbox);
  expect(source.props('disabled')).toBe(false);
  source.vm.$emit('update:checked', true);
  expect(wrapper.emitted('toggleSource')).toEqual([[rule.ruleId, '/fixture/target']]);
  expect(wrapper.text()).not.toContain('cleanup.requiresClose');
  wrapper.unmount();
});

it('uses the checkbox for selection state while retaining the total for partial selection', async () => {
  const ruleWithTwoSources: PresentedScanRuleResult = {
    ...rule,
    sources: [
      { path: '/fixture/first', bytes: 100, fileCount: 1, modifiedAtMs: null, blockReason: null },
      { path: '/fixture/second', bytes: 100, fileCount: 1, modifiedAtMs: null, blockReason: null },
    ],
    sourceCount: 2,
  };
  const wrapper = mount(MdCleanupRuleGroups, {
    props: {
      busy: false,
      leftovers: null,
      rules: [ruleWithTwoSources],
      selectedLeftoverIds: [],
      selectedRuleIds: [],
      sourceSelections: [],
      privilegedScanRuleId: null,
    },
    global: {
      plugins: [createPinia(), i18n],
      stubs: {
        MdResultTable: { methods: { scrollTo() {} }, template: '<div><slot name="header" /><slot /></div>' },
        MdAiAction: true,
      },
    },
  });
  await flushPromises();

  const summary = wrapper.findComponent(MdResultItemContent);
  const checkbox = wrapper.findComponent(MdResultCheckbox);
  expect(summary.props('valueDetail')).toBeUndefined();
  expect(checkbox.props('checked')).toBe(false);

  await wrapper.setProps({ selectedRuleIds: [rule.ruleId] });
  expect(summary.props('valueDetail')).toBeUndefined();
  expect(checkbox.props('checked')).toBe(true);

  await wrapper.setProps({
    sourceSelections: [{ ruleId: rule.ruleId, mode: 'include', paths: ['/fixture/first'] }],
  });
  expect(checkbox.props('indeterminate')).toBe(true);
  expect(summary.props('valueDetail')).toBe(
    i18n.global.t('cleanup.totalSize', { size: ByteSizeService.bytes(ruleWithTwoSources.bytes) })
  );
  wrapper.unmount();
});

it.each([
  ['fr-FR', '1 fichier', '2 fichiers'],
  ['de-DE', '1 Datei', '2 Dateien'],
  ['es-ES', '1 archivo', '2 archivos'],
] as const)('uses the numeric source count for %s plural selection', async (locale, singular, plural) => {
  const previousLocale = i18n.global.locale.value;
  i18n.global.locale.value = locale;
  const withCount = (fileCount: number): PresentedScanRuleResult => ({
    ...rule,
    fileCount,
    sources: [{ ...rule.sources[0]!, fileCount, blockReason: null }],
  });
  const wrapper = mount(MdCleanupRuleGroups, {
    props: {
      busy: false,
      leftovers: null,
      rules: [withCount(2)],
      selectedLeftoverIds: [],
      selectedRuleIds: [],
      sourceSelections: [],
      privilegedScanRuleId: null,
    },
    global: {
      plugins: [createPinia(), i18n],
      stubs: {
        MdResultTable: { methods: { scrollTo() {} }, template: '<div><slot name="header" /><slot /></div>' },
        MdAiAction: true,
        MdIconAction: true,
      },
    },
  });
  try {
    await flushPromises();
    wrapper.findComponent(MdResultItemContent).vm.$emit('toggle');
    await flushPromises();
    expect(wrapper.get('.source-row small').text()).toBe(plural);
    await wrapper.setProps({ rules: [withCount(1)] });
    expect(wrapper.get('.source-row small').text()).toBe(singular);
  } finally {
    wrapper.unmount();
    i18n.global.locale.value = previousLocale;
  }
});
