<script setup lang="ts">
import { useI18n } from 'vue-i18n';

import MdDialogContent from '@/components/custom/md-dialog-content.vue';
import MdDialogHeader from '@/components/custom/md-dialog-header.vue';
import MdDialogFooter from '@/components/custom/md-dialog-footer.vue';
import { Button } from '@/components/ui/button';
import { Dialog, DialogTitle, DialogDescription } from '@/components/ui/dialog';
import type { AnalysisDeleteBatchResult } from '@/lib/models/storage/analysis';
import { FormatService } from '@/lib/services/presentation/format-service';

defineProps<{ open: boolean; result: AnalysisDeleteBatchResult | null }>();
const emit = defineEmits<{ 'update:open': [open: boolean] }>();
const { t, te } = useI18n({ useScope: 'global' });
function failureMessage(reason: string | null) {
  const key = `errorReasons.${reason}.message`;
  return reason && te(key) ? t(key) : t('errors.operationFailed');
}
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <MdDialogContent class="flex min-h-0 flex-col">
      <MdDialogHeader>
        <DialogTitle>{{ t('analysis.deleteCompleted') }}</DialogTitle>
        <DialogDescription>
          {{
            t('analysis.deleteResultSummary', {
              removed: FormatService.integer(result?.removedPaths.length ?? 0),
              failed: FormatService.integer(result?.failed.length ?? 0),
              skipped: FormatService.integer(result?.notAttemptedPaths.length ?? 0),
            })
          }}
        </DialogDescription>
      </MdDialogHeader>
      <div class="max-h-72 overflow-y-auto px-5 scrollbar-stable">
        <p class="mb-3 text-muted-foreground">{{ t('analysis.deleteResultHint') }}</p>
        <ul>
          <li v-for="failure in result?.failed" :key="failure.path" class="border-t border-border py-2">
            <p class="break-all">{{ failure.path }}</p>
            <p class="text-destructive-text">{{ t('analysis.deleteFailed') }} · {{ failureMessage(failure.reason) }}</p>
          </li>
          <li v-for="path in result?.notAttemptedPaths" :key="path" class="border-t border-border py-2">
            <p class="break-all">{{ path }}</p>
            <p class="text-muted-foreground">{{ t('analysis.deleteNotAttempted') }}</p>
          </li>
        </ul>
      </div>
      <MdDialogFooter>
        <Button variant="outline" @click="emit('update:open', false)">{{ t('common.close') }}</Button>
      </MdDialogFooter>
    </MdDialogContent>
  </Dialog>
</template>
