<script setup lang="ts">
/**
 * 文件夹列表：**一层，平铺**。
 *
 * 产品口径（用户原话）：「我们应该没有子文件夹这种逻辑，就是只有一层的文件夹可以归档」。
 * 所以这里不再有缩进、不再有"在这下面新建"、也不再有"移动到某个父级" ——
 * 那三样都是层级 UI，留着就等于口径没落地。
 *
 * 为什么不直接忽略 `parentId`：库里可能已经有历史嵌套（本机开发数据就有），
 * 只渲染 depth 0 会让那些子层**连同里面的笔记一起消失**。所以取的是
 * "把整片森林摊平成一排"这一种：一个都不少，只是不再表达层级。
 * 摊平后名字用路径（`父 / 子`）显示，历史子层还认得出来。
 *
 * 删除不级联删笔记（由本地核心保证：笔记移入默认本，只给文件夹本身写一条墓碑）。
 */
import { computed, nextTick, ref } from 'vue';
import type { ComponentPublicInstance } from 'vue';
import AppPopover from './ui/AppPopover.vue';
import { useFolderStore } from '../stores/folders';
import { useNoteStore } from '../stores/notes';
import { useShellStore } from '../stores/shell';
import { t } from '../i18n';
import type { FolderNode } from '../api/types';
import AppIcon from './ui/AppIcon.vue';
import { FOLDER_SWATCHES, dotStyle } from '../util/folderColors';

/** 色点的那些尺寸/描边写在内联而不是 <style> 里：颜色本身来自数据，形状是同一套。
 *  描边是必须的 —— 没有它，低饱和那几支在浅色主题上会直接融进背景。 */
const DOT_SHAPE = {
  width: '10px',
  height: '10px',
  'border-radius': '50%',
  border: '1px solid var(--line-strong)',
  display: 'inline-block',
  'flex': '0 0 auto',
} as const;

/** 挑一支（或清掉）：先收色板再发命令 —— 命令要等一会儿才回，面板留在屏幕上会显得没反应。 */
async function pickColor(id: string, color: string | null, close: () => void): Promise<void> {
  close();
  await folders.setColor(id, color);
}

const folders = useFolderStore();
const notes = useNoteStore();
const shell = useShellStore();

const editingId = ref<string | null>(null);
const nameDraft = ref('');
const inputEl = ref<HTMLInputElement | null>(null);

const rows = computed(() =>
  [...folders.flat]
    .map((entry) => ({ node: entry.node, label: entry.path.length > 1 ? entry.path.join(' / ') : entry.node.name }))
    .sort((a, b) => a.label.localeCompare(b.label, 'zh-CN')),
);

/**
 * 核心对 `systemKind` 非空的文件夹一律拒绝改名/移动/删除（`assert_folder_writable`）。
 * 按钮的可见性跟着核心的判据走，而不是另立一套"看起来像默认本"的猜测
 * —— "默认"只是用户可改的显示名。
 */
function isSystem(node: FolderNode): boolean {
  return typeof node.systemKind === 'string' && node.systemKind.length > 0;
}

async function beginRename(id: string, current: string): Promise<void> {
  editingId.value = id;
  nameDraft.value = current;
  await nextTick();
  inputEl.value?.focus();
  inputEl.value?.select();
}

async function commitRename(id: string): Promise<void> {
  if (editingId.value !== id) return;
  const name = nameDraft.value.trim();
  editingId.value = null;
  if (name.length === 0) return;
  await folders.rename(id, name);
}

async function removeFolder(id: string): Promise<void> {
  await folders.remove(id);
  if (notes.mode.kind === 'folder' && notes.mode.folderId === id) await notes.setMode({ kind: 'all' });
  else await notes.load();
}

function openFolder(id: string | null): void {
  void notes.setMode(id === null ? { kind: 'all' } : { kind: 'folder', folderId: id });
  // 点文件夹也是"回列表"的动作：停在设置页时它以前只改 mode，界面却一动不动；
  // 窄屏停在编辑器那一面时同理（落点收在 shell.openList 一处）。
  shell.openList();
}

function isActive(id: string | null): boolean {
  if (shell.view !== 'workspace') return false;
  if (notes.mode.kind !== 'folder') return false;
  return notes.mode.folderId === id;
}

function rowRef(el: Element | ComponentPublicInstance | null): void {
  if (el instanceof HTMLInputElement) inputEl.value = el;
}
</script>

<template>
  <ul class="tree" role="list" data-testid="folder-list">
    <li v-for="row in rows" :key="row.node.id" class="tree__item">
      <div class="tree__row" :data-active="isActive(row.node.id) ? 'true' : 'false'" :data-depth="String(row.node.parentId ? 1 : 0)" data-testid="folder-row">
        <button type="button" class="tree__name" :data-testid="`folder-${row.node.id}`" @click="openFolder(row.node.id)">
          <!-- §6「颜色」：用户打过标记的那一本，名字前面一颗小色点（只在有颜色时出现）。
               `aria-hidden`：颜色是给眼睛的辅助，导航靠的是名字本身，读屏念名字就够。 -->
          <span
            v-if="row.node.color"
            class="tree__dot"
            :data-testid="`folder-dot-${row.node.id}`"
            :style="{ ...DOT_SHAPE, ...dotStyle(row.node.color) }"
            aria-hidden="true"
          />
          <span class="tree__label" :title="row.label">{{ row.label }}</span>
          <!-- 空文件夹不显示计数：0 传达不了"这里有没有东西"，只多一个噪点。 -->
          <span v-if="typeof row.node.noteCount === 'number' && row.node.noteCount > 0" class="tree__count">{{ row.node.noteCount }}</span>
        </button>

        <div class="tree__tools">
          <button v-if="!isSystem(row.node)" type="button" class="btn btn--quiet btn--icon" data-testid="folder-rename" :title="t('sidebar.rename')" :aria-label="t('sidebar.rename')" @click="beginRename(row.node.id, row.node.name)">
            <AppIcon :size="16" name="pencil" />
          </button>
          <!-- 色板的入口用一颗"当前色的点"当图标：§2.4 那份图标清单里没有"调色板"这一枚，
               我不会为了让按钮存在而先画一颗没规格的书挡图标（那是 §8 第三问禁的那件事）。
               面板走 AppPopover：一打开就把下面每一排顶走的色板，等于让用户的手指落到别的行为上
               —— 这一条改名框已经栽过（间距 44,44 → 96,44），不该在同一列里再犯一次。
               这颗按钮**对所有本子都在**（包括改名/删除都不许碰的内置那一本）：用户 2026-10-09 拍的
               例外只管颜色这一条，所以这里的可见性判据跟着那条例外走，而不是跟着 `isSystem` 走 ——
               核心那边同样只在"在回收站里"那一侧拒。 -->
          <AppPopover
            testid="folder-color-toggle"
            align="end"
            :label="t('sidebar.colorTitle')"
          >
            <template #glyph>
              <span class="tree__dot" :style="{ ...DOT_SHAPE, ...dotStyle(row.node.color) }" />
            </template>
            <template #default="{ close }">
              <!-- 色板：八支 + "不用颜色"。按钮自己是触摸目标（内联那颗圆点只是样本），
                   `aria-pressed` 说当前选中哪一支 —— 颜色不能是"只有眼睛看得见的选择"。 -->
              <div class="tree__swatches" data-testid="folder-color-panel">
                <span class="tree__swatch-row">
                  <button
                    v-for="s in FOLDER_SWATCHES"
                    :key="s.hex"
                    type="button"
                    class="btn btn--quiet btn--icon tree__swatch"
                    :data-testid="`folder-swatch-${s.hex}`"
                    :aria-label="t(s.labelKey)"
                    :title="t(s.labelKey)"
                    :aria-pressed="row.node.color === s.hex ? 'true' : 'false'"
                    @click="pickColor(row.node.id, s.hex, close)"
                  >
                    <span class="tree__dot" :style="{ ...DOT_SHAPE, ...dotStyle(s.hex) }" />
                  </button>
                </span>
                <button
                  type="button"
                  class="btn btn--quiet btn--block"
                  data-testid="folder-color-none"
                  :aria-pressed="row.node.color ? 'false' : 'true'"
                  @click="pickColor(row.node.id, null, close)"
                >{{ t('sidebar.colorNone') }}</button>
              </div>
            </template>
          </AppPopover>
          <AppPopover
            v-if="!isSystem(row.node)"
            icon="trash"
            testid="folder-delete"
            :label="t('sidebar.deleteFolder')"
          >
            <template #default="{ close }">
              <p class="tree__confirm-text">{{ t('sidebar.deleteFolderHint', { folder: folders.defaultNode?.node.name ?? '' }) }}</p>
              <span class="tree__confirm-row">
                <button type="button" class="btn btn--danger" data-testid="folder-delete-confirm" @click="close(); removeFolder(row.node.id)">{{ t('list.confirm') }}</button>
                <button type="button" class="btn btn--quiet" @click="close()">{{ t('list.cancel') }}</button>
              </span>
            </template>
          </AppPopover>
        </div>
      </div>

      <input
        v-if="editingId === row.node.id"
        :ref="rowRef"
        v-model="nameDraft"
        class="input tree__input"
        type="text"
        data-testid="folder-rename-input"
        :aria-label="t('sidebar.rename')"
        @keydown.enter.prevent="commitRename(row.node.id)"
        @keydown.escape.prevent="editingId = null"
        @blur="commitRename(row.node.id)"
      />
    </li>

    <li v-if="rows.length === 0" class="tree__empty text-sm text-muted">{{ t('sidebar.folders') }}：0</li>
  </ul>
</template>

<style scoped>
.tree {
  list-style: none;
  margin: 0;
  padding: 0;
}

.tree__row {
  display: flex;
  align-items: center;
  gap: var(--sp-1);
  min-height: var(--touch);
  /* 工具层浮在这一行之上（见 .tree__tools），所以这一行得是它的定位父级。
     注意这里**没有** padding-left：层级不再用缩进表达。 */
  position: relative;
  padding-right: var(--sp-2);
}

.tree__row:hover {
  background: var(--hover);
}

.tree__row[data-active='true'] {
  background: var(--accent-soft);
}

.tree__name {
  flex: 1;
  display: flex;
  align-items: center;
  gap: var(--sp-2);
  min-height: var(--touch);
  text-align: left;
  padding: 0 var(--sp-2);
  border-radius: var(--r-row);
  color: var(--ink);
  cursor: pointer;
  min-width: 0;
}

.tree__label {
  /**
   * 名字**折两行，两行读不完才截断** —— 与列表标题同一形状（§5「不许被静默裁掉」，2026-10-09）。
   * 以前是 `nowrap + ellipsis`：长一点的文件夹名在 1440 也吃掉 186 px 的尾巴，而 ㊻ 看不见它
   * （它祖先那一格 `overflow` 的**声明**说"可滚"，实测推 `scrollLeft` 一动不动）。
   * 这一格本来就带 `:title`（整条路径），但"有出口"不是不改形状的理由 —— 出口管的是极长那一段，
   * 形状管的是绝大多数名字一眼看完。行高跟着内容走：`.tree__item` 是 `min-height: var(--touch)`，
   * 不是固定高度，所以长名字只是把那一排撑到两行，不会压到邻居。
   */
  min-width: 0;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: normal;
  overflow-wrap: break-word;
}

.tree__count {
  font-size: var(--text-xs);
  color: var(--mute);
}

.tree__tools {
  /**
   * 工具层**浮在行上**，不参与这一行的排版。三个理由，缺一不可：
   *  ① 静止态不能再"占 0 宽"或"占 176 宽"：占 0 宽就得配 `overflow: hidden`，
   *     那会把悬浮面板一起剪掉（面板量到 inView=false 就是这么来的）；
   *     占 176 宽则把名字挤成两行 —— 真读数：打开确认层时侧栏 scrollHeight 从 536 掉到 492、
   *     两行往上跳，正是"提示不该在底下占一格"的同一种病，只是换了个方向。
   *  ② 面板要能逃出这一栏的裁剪，所以这里不能有任何 overflow 裁剪。
   *  ③ 静止态要不可点（`pointer-events: none`），否则透明按钮会盖住名字那一片的点击。
   */
  position: absolute;
  top: 0;
  right: var(--sp-2);
  bottom: 0;
  display: flex;
  align-items: center;
  gap: var(--sp-1);
  opacity: 0;
  pointer-events: none;
}

.tree__row:hover .tree__tools,
.tree__row:focus-within .tree__tools,
/* 面板开着的时候不许收回去：指针移到面板上就已经离开这一行了，
   没有这一条的话"确认删除"永远点不到 —— 而 aria-expanded 是 Headless UI 自己维护的，
   不另立一套状态。 */
.tree__tools:has([aria-expanded='true']) {
  opacity: 1;
  pointer-events: auto;
}

/* 触屏没有 hover 这回事：不常驻就是"改名/删除这两颗在手机上根本不存在"
   （实测 `hasTouch` 那一档 `opacity:0` 且 `pointer-events:none` —— 连点都点不到）。
   名字那一片要让出工具占的宽度，否则常驻之后会盖住最后一个字。 */
@media (hover: none) {
  .tree__tools {
    opacity: 1;
    pointer-events: auto;
  }

  .tree__name {
    padding-right: 6.5rem;
  }
}

.tree__confirm-text {
  margin: 0;
  color: var(--body);
}

.tree__confirm-row {
  display: flex;
  gap: var(--sp-1);
}

/* 就地改名那一格：**盖在原位上**，不排在名字下面。
   用户第 ④ 条原话是"在原始的那个内容行里输出，而不是底下突然补充一个新的行"——
   以前这个 input 是 `.tree__row` 的**兄弟**且在流内，点一下 ✎ 会把整个 li 撑高：
   实测三排文件夹的间距从 `44,44` 变成 `96,44`（中间被插进 52 px），下面每一排都往下跳。
   现在 li 是定位父级，input 绝对定位压在名字那一行上（DOM 里它在后面 ⇒ 画在上面，
   工具层那两颗也被它盖住 —— 编辑态不该还能点删除）。 */
.tree__item {
  position: relative;
}

/* 每一排都是定位元素，于是 DOM 里在后的那几排会画在前面那排的面板之上 ——
   色板 140 px 高，中心那一点落到别的行上（`elementFromPoint` 读到的是别行）。
   用 `:focus-within`（Headless UI 打开面板会把焦点移进去）而不是 `:hover`：
   鼠标移开后面板还开着，那时它一样得在最上面。 */
.tree__item:hover,
.tree__item:focus-within {
  z-index: 3;
}

.tree__input {
  position: absolute;
  top: 50%;
  right: var(--sp-3);
  left: var(--sp-3);
  width: auto;
  margin: 0;
  transform: translateY(-50%);
  background: var(--canvas);
}

.tree__empty {
  padding: var(--sp-2) var(--sp-3);
}

/* 色板住在 AppPopover 的面板里（绝对定位，不占版面），这里只管排布。
   八支一行摆不下就换行，但每支仍是 `.btn` 的那颗 44pt 触摸目标 —— 不靠缩小色点来省宽度，
   色点是样本、按钮才是目标。 */
.tree__swatches {
  display: flex;
  flex-direction: column;
  gap: var(--sp-1);
}

.tree__swatch-row {
  display: flex;
  flex-direction: row;
  flex-wrap: wrap;
  gap: var(--sp-1);
}

.tree__swatch {
  padding: var(--sp-2);
}
</style>
