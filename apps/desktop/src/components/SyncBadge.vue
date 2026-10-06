<script setup lang="ts">
/** 同步徽标：用户可见的全部同步语义，只有那几格（协议细节一律折进来）。 */
import { computed } from 'vue';
import { useSyncStore } from '../stores/sync';
import { useShellStore } from '../stores/shell';
import { t } from '../i18n';

const sync = useSyncStore();
const shell = useShellStore();

const GLYPHS: Record<string, string> = {
  synced: '✓',
  syncing: '↻',
  offline: '○',
  failed: '!',
  // 未配置账户：给一个**静止**的字形。转不停的圈会被读成"正在忙"，
  // 而这里的事实是"没在同步" —— 字形必须说出来的话。
  idle: '·',
};

const glyph = computed(() => GLYPHS[sync.badge] ?? '!');
const detailText = computed(() => (sync.detail ? `${sync.label} · ${sync.detail}` : sync.label));
const title = computed(() => {
  if (sync.badge === 'failed') return t('sync.localReady');
  // 静止那两格要说清"点它会去哪儿"，否则这枚控件看着像坏了。
  if (sync.badge === 'idle') return t('sync.idleGoConfigure');
  return sync.label;
});

/**
 * 点徽标：**在配好的时候真同步，没配/关掉的时候把人带去配**。
 *
 * 为什么不沿用"照旧调 syncNow()"：那一调就把徽标点亮成"正在同步"，而
 * 没配账户时核心既不会回错、也没有调度器会发事件来纠正它 ⇒ 那颗圈转到用户重启为止
 * （用户原话：「我没有配置同步咋一点击未同步就开始转了」）。
 * 门控在 `stores/sync.ts` 里已经有一道（别的入口也吃那道），这里只多做一件事：
 * 给这一次点击一个去处，而不是让鼠标落空。
 */
function onClick(): void {
  void sync.syncNow();
  // 门控在 store 里（没配/关掉时它不发请求、也不点亮徽标），这里只补"去处"。
  if (!sync.syncActive) shell.goto('settings');
}
</script>

<template>
  <div class="syncline">
    <button
      type="button"
      class="badge"
      :data-badge="sync.badge"
      :title="title"
      :aria-busy="sync.badge === 'syncing' ? 'true' : 'false'"
      :disabled="sync.busy"
      data-testid="sync-badge"
      @click="onClick()"
    >
      <span class="badge__glyph" aria-hidden="true">{{ glyph }}</span>
      <span>{{ sync.label }}</span>
      <span v-if="sync.percent !== null" class="text-muted">{{ t('sync.progress', { done: sync.percent, total: 100 }) }}</span>
    </button>
    <button v-if="sync.showRetry" type="button" class="btn btn--quiet text-sm" data-testid="sync-retry" @click="sync.syncNow()">
      {{ t('sync.retry') }}
    </button>
    <span class="visually-hidden" role="status" aria-live="polite">{{ detailText }}</span>
    <!-- 那一句"为什么"必须**看得见**。PROXY.md §7 说徽标要"停在『需要凭据』"，
         而核心给的说法（`sync.needs_credentials` 那格文案）以前只进 `aria-live`
         与 `title` ⇒ 屏幕上只剩"离线"两个字，用户既不知道是口令没了、
         也不知道该去哪儿修。静止态更要说清是哪一种静止（未配置 / 已关闭 / 缺凭据）。 -->
    <p v-if="sync.detail" class="syncline__detail" data-testid="sync-detail">{{ sync.detail }}</p>
  </div>
</template>

<style scoped>
.syncline {
  display: flex;
  align-items: center;
  gap: var(--sp-2);
  flex-wrap: wrap;
}

/**
 * 侧栏里的徽标**不该是个胶囊**。
 *
 * `.badge` 的默认样式（pill 圆角 + 描边 + `bg-raised`）是给"独立的一枚状态章"设计的
 * —— 放在页面里很显眼。但它在侧栏里与上面三个 `nav-btn` 视觉语言完全不同：
 * 那些是**无边框的文字行**，它是**带框的胶囊**，于是它不像"侧栏的一行"，
 * 而像"从别处掉进来的东西"（用户反馈"层级乱、看着不齐"）。
 *
 * ⇒ 在侧栏语境下去掉框与底色，改为与 `nav-btn` 同款的左对齐文字行；
 *   状态仍靠字形（✓ / ↻ / ○ / ! / ·）与 `data-badge` 的颜色表达。
 *   **保留可点**（手动同步是保留能力），hover 时给底色作为反馈。
 */
.syncline :deep(.badge) {
  width: 100%;
  justify-content: flex-start;
  min-height: var(--touch);
  padding: 0 var(--sp-3);
  border: 0;
  border-radius: var(--r-row, 6px);
  background: none;
  font-weight: 500;
}

.syncline :deep(.badge:hover) {
  background: var(--hover);
  color: var(--ink);
}

/* 占满一整行：状态那一行下面单独一句"为什么"，不去挤右侧的重试按钮。 */
.syncline__detail {
  flex: 1 0 100%;
  margin: 0;
  padding: 0 var(--sp-3) var(--sp-1);
  font-size: var(--text-xs);
  line-height: var(--leading-body);
  color: var(--mute);
}
</style>
