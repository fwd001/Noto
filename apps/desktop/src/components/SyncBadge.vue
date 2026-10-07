<script setup lang="ts">
/** 同步徽标：用户可见的全部同步语义，只有那几格（协议细节一律折进来）。 */
import { computed, onMounted, watch } from 'vue';
import AppIcon from './ui/AppIcon.vue';
import type { IconName } from './ui/icons';
import { useSyncStore } from '../stores/sync';
import { useShellStore } from '../stores/shell';
import { t } from '../i18n';
import { SYNC_ICONS } from './ui/icons';

const sync = useSyncStore();
const shell = useShellStore();

const iconName = computed<IconName>(() => SYNC_ICONS[sync.shownBadge] ?? 'sync-failed');
const detailText = computed(() => (sync.detail ? `${sync.label} · ${sync.detail}` : sync.label));
const title = computed(() => {
  if (sync.shownBadge === 'failed') return t('sync.localReady');
  // 静止那三格要说清"点它会去哪儿"，否则这枚控件看着像坏了。
  if (sync.shownBadge === 'idle') return t('sync.idleGoConfigure');
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
  // 门控在 store 里（没配 / 关掉 / 这一轮没口令时它不发请求、也不点亮徽标），这里只补"去处"。
  // 第三句那一格点了要能**走到重填的那一格** —— 只把徽标说清、不给出口，等于把人留在原地读标语。
  if (!sync.syncActive || sync.idleReason === 'password') shell.goto('settings');
}

/** 开机问一次，一轮跑完再问一次 —— "上一次成功"必须是核心那个**持久**的时间，不是本次会话凑的。 */
onMounted(() => {
  void sync.refreshStatus();
});

watch(
  () => sync.badge,
  (_next: string, prev: string) => {
    // 一轮跑完就要重新问一次持久事实：`synced`/`failed` 之外还有**静止那一格**（§4.3 让路、
    // §4.2 只读、§4.3 G87 停下等确认）—— 那些都不报成败，但都带着界面要显示的事实。
    if (prev === 'syncing') void sync.refreshStatus();
  },
);
</script>

<template>
  <!-- §3.2：这一块是**状态陈述 + 可点动作**，不是提示条；它常驻，不跟文件夹列表一起滚。
       §4.3：五种事实一句不许少，"上一次：{时间}"只在真拿到时间时出现。 -->
  <div class="syncbar" :data-badge="sync.shownBadge" data-testid="syncbar">
    <button
      type="button"
      class="syncbar__title"
      :data-badge="sync.shownBadge"
      :title="title"
      :aria-busy="sync.shownBadge === 'syncing' ? 'true' : 'false'"
      :disabled="sync.busy"
      data-testid="sync-badge"
      @click="onClick()"
    >
      <AppIcon class="syncbar__glyph" :name="iconName" :data-spin="sync.shownBadge === 'syncing' ? 'true' : 'false'" />
      <span>{{ sync.label }}</span>
      <span v-if="sync.percent !== null" class="syncbar__progress">{{ t('sync.progress', { done: sync.percent, total: 100 }) }}</span>
    </button>
    <p v-if="sync.lastSuccessLine" class="syncbar__when" data-testid="sync-last-success">{{ sync.lastSuccessLine }}</p>
    <button v-if="sync.showRetry" type="button" class="syncbar__action" data-testid="sync-retry" @click="sync.syncNow()">
      {{ t('sync.retry') }}
    </button>
    <span class="visually-hidden" role="status" aria-live="polite">{{ detailText }}</span>
    <!-- 那一句"为什么"必须**看得见**。PROXY.md §7 说徽标要"停在『需要凭据』"，
         而核心给的说法（`sync.needs_credentials` 那格文案）以前只进 `aria-live`
         与 `title` ⇒ 屏幕上只剩"离线"两个字，用户既不知道是口令没了、
         也不知道该去哪儿修。静止态更要说清是哪一种静止（未配置 / 已关闭 / 缺凭据）。 -->
    <p v-if="sync.detail" class="syncbar__desc" data-testid="sync-detail">{{ sync.detail }}</p>
  </div>
</template>

<style scoped>
/** 设计稿 §3.2 的 `.syncbar`：sunken 底的一小块，标题行 / 时间 / 原因 / 动作自上而下。 */
.syncbar {
  display: flex;
  flex-direction: column;
  gap: var(--sp-1);
  margin: var(--sp-2) 0 0;
  padding: var(--sp-3);
  border-radius: var(--r-row);
  background: var(--sunken);
}

.syncbar__title {
  display: flex;
  align-items: center;
  gap: 7px;
  min-height: var(--touch);
  padding: 0;
  border: 0;
  background: none;
  color: var(--ink);
  font-weight: 600;
  font-size: var(--text-sm);
  text-align: left;
  cursor: pointer;
}

.syncbar__glyph {
  width: 16px;
  height: 16px;
  flex: 0 0 16px;
}

/* §1.6 / §2.3：五格里**只有"正在同步"会转**，第五格绝对静止。1.4s linear infinite。
   `@keyframes sync-spin` 在 `styles/base.css` 里（全局）：底栏那颗 tab 也要转同一圈，
   放在这个 scoped 块里会被改名（`sync-spin-data-v-…`），全局那条引用就静默不转了。 */
.syncbar__glyph[data-spin='true'] {
  animation: sync-spin 1.4s linear infinite;
  transform-origin: 50% 50%;
}

/* 五格各自的颜色（§2.3）：只有"正在同步"会动，第五格绝对静止 */
.syncbar__title[data-badge='synced'] .syncbar__glyph { color: var(--ok); }
.syncbar__title[data-badge='syncing'] .syncbar__glyph { color: var(--accent); }
.syncbar__title[data-badge='failed'] .syncbar__glyph { color: var(--danger); }
.syncbar__title[data-badge='offline'] .syncbar__glyph,
.syncbar__title[data-badge='idle'] .syncbar__glyph { color: var(--mute); }

.syncbar__progress {
  color: var(--mute);
  font-weight: 400;
}

.syncbar__when {
  margin: 0;
  font-size: var(--text-xs);
  line-height: 1.5;
  color: var(--mute);
}

.syncbar__desc {
  margin: 0;
  font-size: var(--text-xs);
  line-height: 1.5;
  color: var(--body);
}

.syncbar__action {
  align-self: flex-start;
  min-height: 28px;
  padding: 0 var(--sp-3);
  border: 0;
  border-radius: var(--r-chip);
  background: var(--canvas);
  color: var(--accent);
  font-weight: 600;
  font-size: var(--text-xs);
  cursor: pointer;
}
</style>
