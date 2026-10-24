<script setup lang="ts">
import { computed } from 'vue';
import { ConfigProvider } from 'reka-ui';

import { Toaster } from '@/components/ui/sonner';
import { TooltipProvider } from '@/components/ui/tooltip';
import MdAppShell from '@/layouts/md-app-shell.vue';
import { TOOLTIP_OPEN_DELAY_MS } from '@/lib/models/presentation/ui';
import type { AppSettings } from '@/lib/models/settings/settings';
import { useAppStore } from '@/stores/app-store';

const appStore = useAppStore();
const toastTheme = computed<AppSettings['theme']>(() => appStore.settings.theme);
const toastOffset = { top: 'calc(var(--layout-page-header-height) + 12px)', left: '16px', right: '16px' };
// Reuse Tauri's per-document style nonce for dynamically rendered UI styles.
const styleNonce = document.head.querySelector<HTMLStyleElement>('style[nonce]')?.nonce || undefined;
</script>

<template>
  <ConfigProvider :nonce="styleNonce">
    <TooltipProvider
      :delay-duration="TOOLTIP_OPEN_DELAY_MS"
      :disable-hoverable-content="true"
      :ignore-non-keyboard-focus="true"
    >
      <MdAppShell />
    </TooltipProvider>
    <!-- Keep feedback below the header and away from bottom action bars. -->
    <Toaster
      :theme="toastTheme"
      position="top-center"
      :offset="toastOffset"
      :mobile-offset="toastOffset"
      :gap="10"
      :visible-toasts="1"
      rich-colors
      close-button
    />
  </ConfigProvider>
</template>
