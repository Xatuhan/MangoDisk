// @vitest-environment happy-dom

import { mount } from '@vue/test-utils';
import { createI18n } from 'vue-i18n';
import { describe, expect, it, vi } from 'vitest';
import zh from '@/locales/zh-CN.json';
import type { ApplicationDate } from '@/lib/models/applications/application';
import MdApplicationDate from './md-application-date.vue';

const value: ApplicationDate = {
  timestampMs: Date.UTC(2026, 8, 17, 12),
  source: 'preferencesModified',
  precision: 'timestamp',
  capturedAtMs: Date.UTC(2026, 9, 23),
  estimated: true,
  partial: false,
};

function render(date: ApplicationDate | null, readStatus: 'complete' | 'partial' = 'complete', realTooltip = false) {
  return mount(MdApplicationDate, {
    attachTo: realTooltip ? document.body : undefined,
    props: { value: date, kind: 'lastUsed', readStatus },
    global: {
      plugins: [createI18n({ legacy: false, locale: 'zh-CN', messages: { 'zh-CN': zh } })],
      stubs: realTooltip ? {} : { MdTooltip: { props: ['text'], template: '<span :data-hint="text"><slot /></span>' } },
    },
  });
}

describe('application date evidence', () => {
  it('keeps estimates out of the visible date and uses the date itself as a focusable trigger', () => {
    const wrapper = render(value);
    expect(wrapper.text()).toContain('2026');
    expect(wrapper.text()).not.toContain('估算');
    expect(wrapper.get('[tabindex="0"]').text()).toBe(wrapper.text());
    expect(wrapper.get('[tabindex="0"]').attributes('aria-label')).toContain('日期说明');
    expect(wrapper.find('button, svg').exists()).toBe(false);
    expect(wrapper.get('[data-hint]').attributes('data-hint')).toContain('应用配置文件修改日期');
    expect(wrapper.get('[data-hint]').attributes('data-hint')).toContain('估算');
    expect(wrapper.get('[tabindex="0"]').attributes('title')).toBeUndefined();
  });

  it('shows the evidence when hovering over the date text', async () => {
    const wrapper = render(value, 'complete', true);
    try {
      await wrapper.get('.application-date-trigger').trigger('pointermove', { pointerType: 'mouse' });
      await vi.waitFor(() =>
        expect(document.querySelector('[role="tooltip"]')?.textContent).toContain('应用配置文件修改日期')
      );
    } finally {
      wrapper.unmount();
    }
  });

  it('distinguishes missing evidence from an incomplete read', () => {
    const complete = render(null);
    expect(complete.text()).toBe('暂无记录');
    expect(complete.find('[tabindex]').exists()).toBe(false);
    const partial = render(null, 'partial');
    expect(partial.get('[data-hint]').attributes('data-hint')).toContain('范围限制');
  });

  it('shows bounded provenance without treating reported usage as an estimate', () => {
    const wrapper = render({ ...value, source: 'biomeFocus', estimated: false, partial: true });
    const hint = wrapper.get('[data-hint]').attributes('data-hint');
    expect(hint).toContain('本机系统前台活动历史');
    expect(hint).toContain('范围限制');
    expect(hint).not.toContain('根据应用关联文件变化估算');
  });
});
