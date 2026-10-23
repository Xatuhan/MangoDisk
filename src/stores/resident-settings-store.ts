import { defineStore } from 'pinia';
import type { ResidentPreferences } from '@/lib/models/resident/resident';
import { ResidentService } from '@/lib/services/resident/resident-service';
import { LoggerService } from '@/lib/services/diagnostics/logger-service';
import { parseCommandError, parseCommandErrorReason } from '@/lib/utils/diagnostics/error';

/** Rebase disjoint edits only; arrays are atomic fields so competing choices cannot be silently combined. */
function rebase(
  base: ResidentPreferences,
  draft: ResidentPreferences,
  latest: ResidentPreferences
): ResidentPreferences | null {
  if (latest.schemaVersion !== base.schemaVersion || latest.revision <= base.revision) return null;
  const patch: Partial<ResidentPreferences> = {};
  for (const key of Object.keys(draft) as (keyof ResidentPreferences)[]) {
    if (key === 'revision' || key === 'schemaVersion') continue;
    const before = JSON.stringify(base[key]);
    const desired = JSON.stringify(draft[key]);
    if (before === desired) continue;
    const current = JSON.stringify(latest[key]);
    if (current !== before && current !== desired) return null;
    Object.assign(patch, { [key]: draft[key] });
  }
  return copy({ ...latest, ...patch });
}

function copy(value: ResidentPreferences): ResidentPreferences {
  return {
    ...value,
    floating: { ...value.floating },
    fanIds: [...value.fanIds],
    metrics: value.metrics.map(metric => ({ ...metric })),
    colorRules: value.colorRules.map(rule => ({ ...rule, rules: rule.rules.map(condition => ({ ...condition })) })),
  };
}

export const useResidentSettingsStore = defineStore('resident-settings', {
  state: () => ({
    preferences: null as ResidentPreferences | null,
    draft: null as ResidentPreferences | null,
    loading: false,
    saving: false,
    error: false,
    editRevision: 0,
  }),
  actions: {
    async load() {
      if (this.loading || this.saving) return;
      this.loading = true;
      this.error = false;
      const revision = this.editRevision;
      try {
        const value = await ResidentService.preferences();
        // A retry can overlap an edit made from the retained draft. Its older
        // response must not replace the newer optimistic or committed state.
        if (revision !== this.editRevision) return;
        if (value.schemaVersion !== 38) throw new Error('unsupported resident preferences');
        this.preferences = value;
        this.draft = copy(value);
      } catch (error) {
        if (revision !== this.editRevision) return;
        this.error = true;
        LoggerService.warn('resident', 'preferences_load_failed', { error });
      } finally {
        this.loading = false;
      }
    },
    async change(patch: Partial<ResidentPreferences>) {
      if (!this.draft || !this.preferences) return;
      this.draft = copy({ ...this.draft, ...patch });
      this.editRevision += 1;
      if (this.saving) return;
      this.saving = true;
      this.error = false;
      let recoveredConflict = false;
      try {
        while (this.draft && this.preferences) {
          const revision = this.editRevision;
          const requested = copy({ ...this.draft, revision: this.preferences.revision });
          try {
            const committed = await ResidentService.savePreferences(requested);
            if (committed.schemaVersion !== 38) throw new Error('unsupported resident preferences');
            this.preferences = committed;
            if (revision === this.editRevision) {
              this.draft = copy(committed);
              break;
            }
            // Merge rapid edits into the next write using the returned native
            // revision. No older request is allowed to race the latest choice.
          } catch (error) {
            try {
              const latest = await ResidentService.preferences();
              const commandError = parseCommandError(error);
              const merged: ResidentPreferences | null =
                !recoveredConflict &&
                commandError?.details.operation === 'resident_save_preferences' &&
                parseCommandErrorReason(commandError) === 'preferencesConflict' &&
                this.draft
                  ? rebase(this.preferences, this.draft, latest)
                  : null;
              if (merged) {
                LoggerService.info('resident', 'preferences_conflict_retry', {
                  requestedRevision: requested.revision,
                  currentRevision: latest.revision,
                  outcome: 'retry_disjoint_edits_once',
                });
                this.preferences = latest;
                this.draft = merged;
                recoveredConflict = true;
                continue;
              }
              this.preferences = latest;
            } catch {
              /* Keep last committed state. */
            }
            this.error = true;
            this.draft = this.preferences ? copy(this.preferences) : null;
            LoggerService.warn('resident', 'preferences_save_rejected', { revision: requested.revision, error });
            break;
          }
        }
      } finally {
        this.saving = false;
      }
    },
  },
});
