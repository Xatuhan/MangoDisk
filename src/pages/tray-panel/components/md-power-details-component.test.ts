// @vitest-environment happy-dom
import { mount } from '@vue/test-utils';
import { createI18n } from 'vue-i18n';
import { describe, expect, it } from 'vitest';
import en from '@/locales/en-US.json';
import zh from '@/locales/zh-CN.json';
import tw from '@/locales/zh-TW.json';
import ja from '@/locales/ja-JP.json';
import ko from '@/locales/ko-KR.json';
import de from '@/locales/de-DE.json';
import fr from '@/locales/fr-FR.json';
import es from '@/locales/es-ES.json';
import pt from '@/locales/pt-BR.json';
import ru from '@/locales/ru-RU.json';
import tr from '@/locales/tr-TR.json';
import type { BatteryDetails } from '@/lib/models/system-resources/power';
import MdIcon from '@/components/icons/md-icon.vue';
import { emptyReadings } from '@/lib/utils/system-resources/system-resources';
import Page from './md-power-details.vue';

const details: BatteryDetails = {
  chargePercent: 72,
  externalConnected: true,
  chargeState: 'charging',
  secondsToEmpty: null,
  secondsToFull: 4800,
  healthPercent: 84,
  cycleCount: 245,
  capacity: { current: 5200, fullCharge: 7218, design: 8579, unit: 'milliampHours' },
  voltageVolts: 12.77,
  currentMilliamps: 640,
  adapter: { inputWatts: 35.1, reportedWatts: 140, voltageVolts: 28 },
};
function reading() {
  const value = emptyReadings();
  value.observedAtMs = 10000;
  value.batteryPresent = true;
  value.batteryDetails = { status: 'ready', sampledAtMs: 10000, value: structuredClone(details) };
  value.batteryPower = {
    status: 'ready',
    sampledAtMs: 10000,
    value: { watts: 8.2, state: 'charging', source: 'appleSmartBattery' },
  };
  value.batteryTemperature = {
    status: 'ready',
    sampledAtMs: 10000,
    value: { celsius: 31, source: 'appleSmartBattery' },
  };
  value.systemPower = { status: 'ready', sampledAtMs: 10000, value: { watts: 26.4 } };
  return value;
}
describe.each([
  ['en-US', en],
  ['zh-CN', zh],
  ['zh-TW', tw],
  ['ja-JP', ja],
  ['ko-KR', ko],
  ['de-DE', de],
  ['fr-FR', fr],
  ['es-ES', es],
  ['pt-BR', pt],
  ['ru-RU', ru],
  ['tr-TR', tr],
] as const)('power detail presentation (%s)', (locale, messages) => {
  function render(value = reading()) {
    return mount(Page, {
      props: { reading: value, active: true },
      global: {
        plugins: [createI18n({ legacy: false, locale, messages: { [locale]: messages } })],
        stubs: { MdResourceTrend: true, MdTooltip: { template: '<span><slot /></span>' } },
      },
    });
  }
  it('separates native power sources and exposes every battery fact without disclosure', () => {
    const wrapper = render();
    const text = wrapper.text();
    for (const label of [
      messages.power.systemPower,
      messages.power.intoBattery,
      messages.power.inputPower,
      messages.power.reportedPower,
    ])
      expect(text).toContain(label);
    for (const watts of [26.4, 8.2, 35.1, 140])
      expect(text).toContain(new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }).format(watts));
    expect(wrapper.find('details').exists()).toBe(false);
    expect(wrapper.findAll('.battery-facts .resource-fact')).toHaveLength(9);
    expect(wrapper.findAll('.adapter-facts .resource-fact')).toHaveLength(3);
    for (const key of [
      'current',
      'fullCharge',
      'design',
      'batteryVoltage',
      'batteryCurrent',
      'adapterVoltage',
    ] as const)
      expect(text).toContain(messages.power[key]);
    expect(text).toContain('mAh');
    expect(text).toContain('84');
    wrapper.unmount();
  });
  it('shows a connected full battery without a fictitious charging-time estimate', () => {
    const value = reading();
    value.batteryDetails.value!.chargeState = 'full';
    value.batteryDetails.value!.secondsToFull = null;
    const wrapper = render(value);
    expect(wrapper.text()).toContain(messages.power.states.full);
    expect(wrapper.find('.battery-state small').exists()).toBe(false);
    expect(wrapper.text()).not.toContain('65535');
    wrapper.unmount();
  });
  it('does not imply an active estimate when the OS provides no charging time', () => {
    const value = reading();
    value.batteryDetails.value!.secondsToFull = null;
    const wrapper = render(value);
    expect(wrapper.text()).toContain(messages.power.states.charging);
    expect(wrapper.find('.battery-state small').exists()).toBe(false);
    wrapper.unmount();
  });
  it('preserves Windows energy-capacity units and missing current', () => {
    const value = reading();
    value.batteryDetails.value!.capacity!.unit = 'milliwattHours';
    value.batteryDetails.value!.currentMilliamps = null;
    const wrapper = render(value);
    expect(wrapper.text()).toContain('mWh');
    expect(wrapper.text()).not.toContain('mAh');
    expect(wrapper.text()).toContain('—');
    wrapper.unmount();
  });
  it('omits unsupported system meters and adapters without native facts', async () => {
    const value = reading();
    value.batteryDetails.value!.adapter = null;
    const wrapper = render(value);
    await wrapper.setProps({ systemPowerSupported: false });
    expect(wrapper.find('.system-power').exists()).toBe(false);
    expect(wrapper.text()).not.toContain(messages.power.adapter);
    expect(wrapper.text()).toContain(messages.power.details);
    wrapper.unmount();
  });
  it('keeps system power on desktops without a battery section', () => {
    const value = reading();
    value.batteryPresent = false;
    const wrapper = render(value);
    expect(wrapper.text()).toContain(new Intl.NumberFormat(locale).format(26.4));
    expect(wrapper.find('.battery-status').exists()).toBe(false);
    expect(wrapper.find('details').exists()).toBe(false);
    expect(wrapper.text()).not.toContain(messages.power.adapter);
    wrapper.unmount();
  });
  it('keeps battery and adapter markup unchanged when a background refresh completes', async () => {
    const value = reading();
    value.observedAtMs = 16000;
    value.batteryDetails.status = 'stale';
    value.batteryPower.status = 'stale';
    value.batteryTemperature.status = 'stale';
    const wrapper = render(value);
    const before = ['.battery-status', '.battery-facts', '.adapter-facts'].map(selector =>
      wrapper.get(selector).html()
    );
    const updated = reading();
    updated.observedAtMs = 16000;
    await wrapper.setProps({ reading: updated });
    expect(
      ['.battery-status', '.battery-facts', '.adapter-facts'].map(selector => wrapper.get(selector).html())
    ).toEqual(before);
    wrapper.unmount();
  });
  it('shows previous power and battery facts without a refresh label', () => {
    const value = reading();
    value.observedAtMs = 16000;
    const wrapper = render(value);
    expect(wrapper.text()).toContain(new Intl.NumberFormat(locale).format(26.4));
    expect(wrapper.text()).toContain('245');
    expect(wrapper.find('[role="status"]').exists()).toBe(false);
    expect(wrapper.text()).toContain(messages.power.states.charging);
    wrapper.unmount();
  });
});

describe('power source icon', () => {
  function render(value = reading()) {
    return mount(Page, {
      props: { reading: value, active: true },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'en-US', messages: { 'en-US': en } })],
        stubs: { MdResourceTrend: true, MdTooltip: { template: '<span><slot /></span>' } },
      },
    });
  }
  it.each([
    ['charging', true, 72, 'batteryCharging'],
    ['charging', null, 72, 'batteryCharging'],
    ['charging', false, 72, 'battery'],
    ['full', true, 100, 'plug'],
    ['paused', true, 80, 'plug'],
    ['connected', true, 80, 'plug'],
    ['unknown', true, 80, 'battery'],
    ['unknown', false, 80, 'battery'],
    ['full', false, 100, 'batteryFull'],
    ['discharging', false, 100, 'batteryFull'],
    ['discharging', true, 100, 'batteryFull'],
    ['discharging', null, 100, 'batteryFull'],
    ['discharging', false, 76, 'batteryFull'],
    ['discharging', false, 75, 'batteryMedium'],
    ['discharging', false, 26, 'batteryMedium'],
    ['discharging', false, 25, 'batteryLow'],
    ['discharging', false, 1, 'batteryLow'],
    ['discharging', false, 0, 'battery'],
    ['discharging', false, null, 'battery'],
    ['discharging', false, -1, 'battery'],
    ['discharging', false, 101, 'battery'],
    ['discharging', false, Number.NaN, 'battery'],
  ] as const)('renders %s with external=%s and level=%s as %s', (state, external, percent, expected) => {
    const value = reading();
    Object.assign(value.batteryDetails.value!, {
      chargeState: state,
      externalConnected: external,
      chargePercent: percent,
    });
    const wrapper = render(value);
    expect(wrapper.getComponent(MdIcon).props('name')).toBe(expected);
    expect(wrapper.get('.battery-level svg').attributes('aria-hidden')).toBe('true');
    expect(wrapper.text()).toContain(en.power.states[state]);
    wrapper.unmount();
  });
  it('updates the icon when power changes and retains the previous supply state', async () => {
    const value = reading();
    const wrapper = render(value);
    expect(wrapper.getComponent(MdIcon).props('name')).toBe('batteryCharging');
    const onBattery = structuredClone(value);
    Object.assign(onBattery.batteryDetails.value!, { chargeState: 'discharging', externalConnected: false });
    await wrapper.setProps({ reading: onBattery });
    expect(wrapper.getComponent(MdIcon).props('name')).toBe('batteryMedium');
    const plugged = structuredClone(value);
    Object.assign(plugged.batteryDetails.value!, { chargeState: 'full', chargePercent: 100 });
    await wrapper.setProps({ reading: plugged });
    expect(wrapper.getComponent(MdIcon).props('name')).toBe('plug');
    await wrapper.setProps({ reading: { ...plugged, observedAtMs: 16000 } });
    expect(wrapper.getComponent(MdIcon).props('name')).toBe('plug');
    expect(wrapper.find('[role="status"]').exists()).toBe(false);
    await wrapper.setProps({ reading: emptyReadings() });
    expect(wrapper.getComponent(MdIcon).props('name')).toBe('battery');
    wrapper.unmount();
  });
});

describe('power cache lifecycle', () => {
  function render(value = reading()) {
    return mount(Page, {
      props: { reading: value, active: true },
      global: {
        plugins: [createI18n({ legacy: false, locale: 'en-US', messages: { 'en-US': en } })],
        stubs: { MdResourceTrend: true, MdTooltip: { template: '<span><slot /></span>' } },
      },
    });
  }
  it('retains cached facts on reopen and replaces them when a new observation arrives', async () => {
    const value = reading();
    value.observedAtMs = 70000;
    value.batteryDetails.status = 'stale';
    const wrapper = render(value);
    expect(wrapper.get('.battery-level').text()).toContain('72');
    expect(wrapper.get('.adapter-facts').text()).toContain('35.1');
    expect(wrapper.find('.battery-state [role="status"]').exists()).toBe(false);
    expect(wrapper.get('.battery-state').text()).toContain('1 h 20 min');
    await wrapper.setProps({ active: false });
    await wrapper.setProps({ active: true });
    expect(wrapper.get('.battery-level').text()).toContain('72');
    const updated = reading();
    updated.batteryDetails.value!.chargePercent = 73;
    await wrapper.setProps({ reading: updated });
    expect(wrapper.get('.battery-level').text()).toContain('73');
    expect(wrapper.find('.battery-state [role="status"]').exists()).toBe(false);
    updated.batteryDetails.value!.externalConnected = false;
    updated.batteryDetails.value!.adapter = null;
    await wrapper.setProps({ reading: structuredClone(updated) });
    expect(wrapper.find('.adapter-facts').exists()).toBe(false);
    wrapper.unmount();
  });
  it('retains failure context beside the last facts and clears them on physical removal', async () => {
    const value = reading();
    value.batteryDetails.status = 'failed';
    const wrapper = render(value);
    expect(wrapper.get('.battery-level').text()).toContain('72');
    expect(wrapper.get('.battery-state').text()).toContain(en.systemStatus.failed);
    value.batteryPresent = false;
    await wrapper.setProps({ reading: structuredClone(value) });
    expect(wrapper.find('.battery-status').exists()).toBe(false);
    expect(wrapper.find('.adapter-facts').exists()).toBe(false);
    wrapper.unmount();
  });
});
