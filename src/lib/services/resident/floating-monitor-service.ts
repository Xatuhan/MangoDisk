import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type {
  FloatingItemMeasurement,
  FloatingLayoutMeasurement,
  FloatingSnapshot,
} from '@/lib/models/resident/floating-monitor';
import type { FloatingTypography } from '@/lib/models/resident/floating-appearance';
import type { ResidentPanelPage } from '@/lib/models/resident/resident';

export class FloatingMonitorService {
  static onAppearanceChanged(callback: () => void): Promise<() => void> {
    return listen<{ key: string }>('store://change', event => {
      if (event.payload.key === 'settings') callback();
    });
  }
  static ready(): Promise<FloatingSnapshot | null> {
    return invoke('resident_floating_ready');
  }
  static layout(items: FloatingItemMeasurement[] | null, typography: FloatingTypography): Promise<boolean> {
    const measurement: FloatingLayoutMeasurement = { ...typography, items };
    return invoke('resident_floating_layout', { measurement });
  }
  static onSnapshot(callback: (value: FloatingSnapshot) => void): Promise<() => void> {
    return getCurrentWindow().listen<FloatingSnapshot>('resident-floating-snapshot', event => callback(event.payload));
  }
  static menu(): Promise<void> {
    return invoke('resident_floating_menu');
  }
  static drag(): Promise<void> {
    return invoke('resident_floating_drag');
  }
  static togglePage(page: ResidentPanelPage): Promise<void> {
    return invoke('resident_floating_page', { page });
  }
}
