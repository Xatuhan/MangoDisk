// @vitest-environment happy-dom
import { flushPromises, mount } from '@vue/test-utils';
import { h } from 'vue';
import { expect, it, vi } from 'vitest';
import MdTooltip from './md-tooltip.vue';

it('preserves a standalone trigger and shows a dismissible hint without a native title', async () => {
  const clicked = vi.fn();
  const wrapper = mount(MdTooltip, {
    attachTo: document.body,
    props: { text: 'C:\\Applications\\Example.exe' },
    attrs: { class: 'caller-class', 'aria-label': 'Open location' },
    slots: { default: () => h('button', { onClick: clicked }, 'Open') },
  });
  try {
    const button = wrapper.get('button');
    expect(Array.from(wrapper.element.children)).toEqual([button.element]);
    expect(button.classes()).toContain('caller-class');
    expect(button.attributes('aria-label')).toBe('Open location');
    expect(button.attributes('title')).toBeUndefined();
    await button.trigger('pointermove', { pointerType: 'mouse' });
    await vi.waitFor(() => expect(document.querySelector('[role="tooltip"]')?.textContent).toContain('Example.exe'));
    await button.trigger('click');
    expect(clicked).toHaveBeenCalledTimes(1);
    await flushPromises();
    await vi.waitFor(() => expect(document.querySelector('[role="tooltip"]')).toBeNull());
    await wrapper.setProps({ text: '' });
    expect(wrapper.get('button').element).toBe(button.element);
  } finally {
    wrapper.unmount();
  }
});

it('closes hints and resets pointer state when the resident panel becomes inactive', async () => {
  const wrapper = mount(MdTooltip, {
    attachTo: document.body,
    props: { text: 'Health explanation', active: true, delayDuration: 0 },
    slots: { default: () => h('button', 'Health') },
  });
  try {
    await wrapper.get('button').trigger('pointermove', { pointerType: 'mouse' });
    await vi.waitFor(() => expect(document.querySelector('[role="tooltip"]')).not.toBeNull());
    await wrapper.setProps({ active: false });
    await vi.waitFor(() => expect(document.querySelector('[role="tooltip"]')).toBeNull());
    await wrapper.setProps({ active: true });
    expect(document.querySelector('[role="tooltip"]')).toBeNull();
    await wrapper.get('button').trigger('pointermove', { pointerType: 'mouse' });
    await vi.waitFor(() => expect(document.querySelector('[role="tooltip"]')).not.toBeNull());
  } finally {
    wrapper.unmount();
  }
});

it('closes recycled hints when their text changes by default', async () => {
  const wrapper = mount(MdTooltip, {
    attachTo: document.body,
    props: { text: 'First file', delayDuration: 0 },
    slots: { default: () => h('button', 'File') },
  });
  try {
    await wrapper.get('button').trigger('pointermove', { pointerType: 'mouse' });
    await vi.waitFor(() => expect(document.querySelector('[role="tooltip"]')?.textContent).toContain('First file'));
    await wrapper.setProps({ text: 'Another file' });
    await vi.waitFor(() => expect(document.querySelector('[role="tooltip"]')).toBeNull());
  } finally {
    wrapper.unmount();
  }
});

it('keeps live hints readable across updates while honoring deactivation and empty text', async () => {
  const wrapper = mount(MdTooltip, {
    attachTo: document.body,
    props: { text: 'Sampled 2 seconds', closeOnTextChange: false, delayDuration: 0 },
    slots: { default: () => h('button', 'Coverage') },
  });
  try {
    await wrapper.get('button').trigger('pointermove', { pointerType: 'mouse' });
    await vi.waitFor(() => expect(document.querySelector('[role="tooltip"]')).not.toBeNull());
    await wrapper.setProps({ text: 'Sampled 4 seconds' });
    await vi.waitFor(() =>
      expect(document.querySelector('[role="tooltip"]')?.textContent).toContain('Sampled 4 seconds')
    );
    await wrapper.setProps({ active: false });
    await vi.waitFor(() => expect(document.querySelector('[role="tooltip"]')).toBeNull());
    await wrapper.setProps({ active: true });
    expect(document.querySelector('[role="tooltip"]')).toBeNull();
    await wrapper.get('button').trigger('pointermove', { pointerType: 'mouse' });
    await vi.waitFor(() => expect(document.querySelector('[role="tooltip"]')).not.toBeNull());
    await wrapper.setProps({ text: '' });
    await vi.waitFor(() => expect(document.querySelector('[role="tooltip"]')).toBeNull());
  } finally {
    wrapper.unmount();
  }
});

it('restores a keyboard hint after focus scroll without reopening it after dismissal', async () => {
  const parent = document.createElement('div');
  document.body.append(parent);
  const wrapper = mount(MdTooltip, {
    attachTo: parent,
    props: { text: 'Full file name\n/fixture/full-file-name', multiline: true, delayDuration: 0 },
    slots: { default: () => h('button', 'File') },
  });
  const button = wrapper.get('button').element;
  const matches = button.matches.bind(button);
  vi.spyOn(button, 'matches').mockImplementation(selector => selector === ':focus-visible' || matches(selector));
  const settleFocusScroll = () =>
    new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
  try {
    button.focus();
    await flushPromises();
    parent.dispatchEvent(new Event('scroll'));
    await flushPromises();
    await settleFocusScroll();
    await flushPromises();
    expect(document.querySelector('[role="tooltip"]')?.textContent).toContain('/fixture/full-file-name');

    button.blur();
    await flushPromises();
    expect(document.querySelector('[role="tooltip"]')).toBeNull();
    button.focus();
    await flushPromises();
    await wrapper.get('button').trigger('keydown', { key: 'Escape' });
    await settleFocusScroll();
    await flushPromises();
    expect(document.querySelector('[role="tooltip"]')).toBeNull();

    button.blur();
    button.focus();
    await wrapper.setProps({ active: false });
    await settleFocusScroll();
    await flushPromises();
    expect(document.querySelector('[role="tooltip"]')).toBeNull();
  } finally {
    wrapper.unmount();
    parent.remove();
    vi.restoreAllMocks();
  }
});
