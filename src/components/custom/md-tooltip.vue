<script setup lang="ts">
import { onBeforeUnmount, onDeactivated, ref, watch } from 'vue';
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip';
import { TOOLTIP_OPEN_DELAY_MS } from '@/lib/models/presentation/ui';

defineOptions({ inheritAttrs: false });
const props = withDefaults(
  defineProps<{
    text?: string | null;
    delayDuration?: number;
    multiline?: boolean;
    active?: boolean;
    closeOnTextChange?: boolean;
  }>(),
  { active: true, closeOnTextChange: true, text: undefined, delayDuration: undefined }
);
const open = ref(false);
let focusFrame: number | undefined;
function cancelFocusHint() {
  if (focusFrame !== undefined) cancelAnimationFrame(focusFrame);
  focusFrame = undefined;
}
function openFocusedHint(event: FocusEvent) {
  cancelFocusHint();
  const trigger = event.target;
  if (!(trigger instanceof HTMLElement) || !trigger.matches(':focus-visible')) return;
  // Focusing an offscreen row scrolls its container. Reka closes hints on that
  // scroll, so restore keyboard hints once the browser has settled focus.
  focusFrame = requestAnimationFrame(() => {
    focusFrame = requestAnimationFrame(() => {
      focusFrame = undefined;
      if (document.activeElement === trigger && trigger.matches(':focus-visible') && props.text && props.active) {
        open.value = true;
      }
    });
  });
}
// Recycled rows must close a tooltip before it starts describing another file.
watch(
  () => props.text,
  text => {
    cancelFocusHint();
    if (!text || props.closeOnTextChange) open.value = false;
  },
  { flush: 'sync' }
);
watch(
  () => props.active,
  () => {
    cancelFocusHint();
    open.value = false;
  },
  { flush: 'sync' }
);
onBeforeUnmount(cancelFocusHint);
onDeactivated(() => {
  cancelFocusHint();
  open.value = false;
});
</script>

<template>
  <!-- Standalone resident windows do not inherit the main window's provider. -->
  <TooltipProvider
    :delay-duration="delayDuration ?? TOOLTIP_OPEN_DELAY_MS"
    :disable-hoverable-content="true"
    :ignore-non-keyboard-focus="true"
  >
    <!-- Native hiding may omit pointerleave; remount the trigger to reset hover state. -->
    <Tooltip :key="active === false ? 'inactive' : 'active'" v-model:open="open" :disabled="!text || active === false">
      <!-- Preserve the caller's element, layout, accessible name and handlers. -->
      <TooltipTrigger
        as-child
        v-bind="$attrs"
        @focus="openFocusedHint"
        @blur="cancelFocusHint"
        @pointerdown="cancelFocusHint"
        @keydown.esc="cancelFocusHint"
        ><slot
      /></TooltipTrigger>
      <!-- Portal content does not inherit this wrapper's scoped CSS attributes. -->
      <!-- Natural wrapping avoids balanced lines leaving unused space in descriptions and file paths. -->
      <TooltipContent
        v-if="open"
        :aria-label="closeOnTextChange ? undefined : text || undefined"
        class="max-w-[min(24rem,calc(100vw-24px))] text-left text-wrap [overflow-wrap:anywhere]"
        :class="multiline ? 'whitespace-pre-line' : 'whitespace-normal'"
      >
        <slot name="content">{{ text }}</slot>
      </TooltipContent>
    </Tooltip>
  </TooltipProvider>
</template>
