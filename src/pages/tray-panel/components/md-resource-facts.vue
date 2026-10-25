<script setup lang="ts">
import MdTooltip from '@/components/custom/md-tooltip.vue';
import MdCopyButton from '@/components/custom/md-copy-button.vue';
withDefaults(
  defineProps<{
    active?: boolean;
    layout?: 'tiles' | 'rows';
    facts: {
      key: string;
      label: string;
      value: string;
      unit: string;
      copy?: boolean;
      hint?: string;
      liveHint?: boolean;
    }[];
  }>(),
  { active: true, layout: 'tiles' }
);
</script>
<template>
  <dl v-if="facts.length" class="resource-facts" :class="{ compact: facts.length === 1, rows: layout === 'rows' }">
    <div v-for="fact in facts" :key="fact.key" class="resource-fact">
      <dt>
        <MdTooltip v-if="fact.hint" :active="active" :text="fact.hint" :close-on-text-change="!fact.liveHint" multiline
          ><span tabindex="0">{{ fact.label }}</span></MdTooltip
        >
        <span v-else>{{ fact.label }}</span>
      </dt>
      <dd>
        <MdCopyButton
          v-if="fact.copy && fact.value !== '—'"
          :active="active"
          :text="fact.value"
          :label="fact.label"
          show-text
        />
        <template v-else
          >{{ fact.value }}<small v-if="fact.unit">{{ fact.unit }}</small></template
        >
      </dd>
    </div>
  </dl>
</template>
<style scoped>
@reference "@assets/main.css";
.resource-facts {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(90px, 1fr));
  gap: 8px;
  flex: none;
}
.resource-fact {
  @apply rounded-lg bg-muted/40;
  padding: 8px;
  min-width: 0;
}
dt {
  @apply text-muted-foreground;
  display: flex;
  min-width: 0;
  align-items: center;
  justify-content: space-between;
  gap: 4px;
  font-size: 10px;
}
dt > span:first-child {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
dd {
  margin-top: 6px;
  font-size: 18px;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
}
small {
  margin-left: 3px;
  font-size: 10px;
  font-weight: normal;
}
.compact .resource-fact {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}
.compact dd {
  margin-top: 0;
  flex: none;
}
.rows {
  display: flex;
  flex-direction: column;
  gap: 0;
}
.rows .resource-fact {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 12px;
  padding: 4px 0;
  border-radius: 0;
  background: transparent;
}
.rows dt {
  font-size: 11px;
  flex: none;
}
.rows dd {
  margin: 0;
  font-size: 12px;
  font-weight: 500;
  text-align: right;
  max-width: 72%;
  overflow-wrap: anywhere;
}
</style>
