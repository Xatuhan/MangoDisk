<template>
  <section class="resource-card"><slot /></section>
</template>

<style scoped>
@reference "@assets/main.css";
.resource-card {
  @apply rounded-xl border border-border bg-card;
  box-sizing: border-box;
  padding: 10px 12px;
  min-width: 0;
  flex: none;
}
</style>
