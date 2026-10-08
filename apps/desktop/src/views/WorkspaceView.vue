<script setup lang="ts">
/** 工作区：列表 + 编辑器（编辑器再窄也占满剩余宽度）。 */
import { computed, watch } from 'vue';
import NoteList from '../components/NoteList.vue';
import RichEditor from '../components/RichEditor.vue';
import EmptyState from '../components/EmptyState.vue';
import SkeletonRows from '../components/SkeletonRows.vue';
import AppSelect from '../components/ui/AppSelect.vue';
import { useEditorStore } from '../stores/editor';
import { useFolderStore } from '../stores/folders';
import { useNoteStore } from '../stores/notes';
import { useSettingsStore } from '../stores/settings';
import { useShellStore } from '../stores/shell';
import { drawsTitleBar } from '../platform/caps';
import { t } from '../i18n';
import AppIcon from '../components/ui/AppIcon.vue';

const editor = useEditorStore();
const folders = useFolderStore();
const notes = useNoteStore();
const settings = useSettingsStore();
const shell = useShellStore();

const currentRow = computed(() => notes.rowById(notes.selectedId));
const title = computed(() => currentRow.value?.title || t('editor.untitled'));
/**
 * 「未归类」这一格在产品里**不存在**，可它一直在下拉里（缺口 G96）：
 *  · `list_notes` 把笔记**内连接**到 folders 上 —— 不属于任何文件夹的笔记根本列不出来；
 *  · `create_note` 与 `set_note_folder` 的入参都是非空 uuid（核心 `default_folder_id()` 兜的就是「默认本」）；
 *  · 于是那一格 `value: ''` 选下去必然 `bad_args` ⇒ 一次 400，屏幕上的位置一个字都没变。
 * 真正的"还没整理"就是核心 bootstrap 出来的**默认本**，所以这一格改指它，而不是再画一个做不到的状态。
 */
const defaultFolderId = computed(() => folders.defaultNode?.node.id ?? '');
const folderId = computed(() => currentRow.value?.folderId ?? defaultFolderId.value);
const pinned = computed(() => currentRow.value?.pinned === true);
const canEdit = computed(() => notes.selectedId !== null && !editor.loading);

// 选中即打开：编辑器跟着 selectedId 走，只此一处。
// 之前只有"点列表行"这一条路径会 open()，其它入口（新建、列表自动选中、刷新后恢复）
// 留下一块"标题在、正文空、只显示正在读取本地库"的面板 —— 用户看到的就是卡住。
watch(() => notes.selectedId, (id) => { void editor.open(id); }, { immediate: true });

function moveToFolder(id: string): void {
  const noteId = notes.selectedId;
  if (!noteId) return;
  void notes.moveTo(noteId, id);
}

/** 「移到」那一份选项：**只有真文件夹**。以前头一格是「未归类」(`value: ''`)，
 *  而核心没有"不属于文件夹"这一态（见上面 `defaultFolderId` 那段）⇒ 那颗选了必失败。 */
const folderChoices = computed(() =>
  folders.flat.map((entry) => ({ value: entry.node.id, label: entry.path.join(' / ') })));
</script>

<template>
  <div class="workspace">
    <!-- 侧边栏收起后的**唯一**回来的路（没有自绘那一行时）。
         为什么必须放在这里而不是侧栏内部：折叠那颗按钮原本长在 `SidebarPanel` 里
         （sidebar-collapse），侧栏一收它自己就被 `v-show` 一起藏了 ⇒ 收起来之后
         界面上再没有第二颗能把它叫回来的按钮，用户只能重启应用。
         这不是"少一个按钮"，是**出去之后回不来**（§6「无法返回」那一类）。
         有自绘标题栏时这颗不画 —— 那时唯一的把手在标题栏里，画两颗就是
         「折起之后下面还有一层折起」。 -->
    <button
      v-if="!shell.isCompact && !shell.sidebarOpen && !drawsTitleBar(settings.caps)"
      type="button"
      class="workspace__reopen"
      :aria-label="t('sidebar.expand')"
      :title="t('sidebar.expand')"
      data-testid="open-sidebar"
      @click="shell.toggleSidebar()"
    >
      <AppIcon :size="18" name="menu" />
    </button>
    <NoteList v-if="shell.listVisible" @open="shell.openEditor()" />

    <section v-if="shell.editorVisible" class="pane pane--editor" :aria-label="t('editor.pane')" data-testid="editor-pane">
      <div class="pane-header editor-head">
        <button v-if="shell.isCompact" type="button" class="btn btn--quiet btn--icon" :aria-label="t('mobile.back')" data-testid="back-to-list" @click="shell.backToList()">
          <AppIcon :size="18" name="arrow-back" />
        </button>
        <span class="pane-title" :title="title">{{ title }}</span>

        <span class="editor-head__folder">
          <AppSelect
            :model-value="folderId ?? ''"
            :options="folderChoices"
            :label="t('sidebar.moveTo')"
            :disabled="!canEdit"
            testid="move-folder"
            @update:model-value="moveToFolder($event)"
          />
        </span>

        <!--右侧动作区。`margin-left:auto` 把它推到顶栏右端，
             于是顶栏读起来是"左=这篇是什么/  右=对它做什么"，
             而不是三颗按钮挤在标题旁边（用户反馈"对不齐"）。 -->
        <div class="editor-head__actions">
          <button
            type="button"
            class="btn btn--quiet"
            :disabled="!canEdit"
            :aria-pressed="pinned ? 'true' : 'false'"
            data-testid="toggle-pin"
            @click="notes.selectedId && notes.setPinned(notes.selectedId, !pinned)"
          >
            {{ pinned ? t('list.unpin') : t('list.pin') }}
          </button>
          <button
            type="button"
            class="btn btn--quiet btn--danger"
            :disabled="!canEdit || notes.inTrash"
            :title="t('list.moveToTrash')"
            :aria-label="t('list.moveToTrash')"
            data-testid="trash-note"
            @click="notes.selectedId && notes.moveToTrash(notes.selectedId)"
          >
            {{ t('list.delete') }}
          </button>
        </div>
      </div>

      <SkeletonRows v-if="editor.loading" :rows="4" />

      <RichEditor v-else-if="editor.blocks.length > 0" :key="editor.noteId ?? 'empty'" />

      <EmptyState v-else-if="notes.selectedId === null" icon="pencil" :title="t('list.empty')" :hint="t('state.boot')">
        <button type="button" class="btn btn--primary" data-testid="first-new-note" @click="notes.create(notes.mode.kind === 'folder' ? notes.mode.folderId : null)">
          {{ t('list.newNote') }}
        </button>
      </EmptyState>

      <div v-else class="editor-blank" role="status">
        <p class="text-sm text-muted">{{ editor.saveErrorKey ? t('error.fallback') : t('state.loading') }}</p>
        <button type="button" class="btn" data-testid="editor-retry" @click="notes.selectedId && editor.open(notes.selectedId, true)">{{ t('sync.retry') }}</button>
      </div>
    </section>
  </div>
</template>

<style scoped>
.workspace {
  position: relative;
  display: flex;
  flex: 1;
  min-width: 0;
  min-height: 0;
  gap: var(--sp-1);
}

/* 侧栏收起后留在原位的那颗「展开」：绝对定位在列表左缘，
   不占flex 位（否则会把列表挤窄一格，松手时列表宽度会跳一下）。

   ⚠️ 它会盖住列表第一行的标题左端（实测截图里"笔记"两个字被压住了）。
   所以给列表留出等宽的左内边距 —— 让"标题/搜索框"从按钮右边开始，
   而不是从按钮底下钻出来。收起态下这块内边距是唯一的补偿。 */
.workspace__reopen {
  position: absolute;
  top: var(--sp-2);
  left: var(--sp-2);
  z-index: 3;
  width: 36px;
  height: 36px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: 1px solid var(--line);
  border-radius: 8px;
  background: var(--canvas);
  color: var(--body);
  cursor: pointer;
  box-shadow: 0 1px 2px rgb(0 0 0 / 0.12);
}

.workspace__reopen:hover {
  background: var(--hover);
  color: var(--ink);
}

/* 收起态：给列表顶上一块与按钮等高的留白，标题不被压住。
   挂在 workspace 上而不是 NoteList 内部：这是"侧栏收起"这个状态的补偿，
   属于布局层，不该由列表自己去猜侧栏的状态。
   选择器对的是 NoteList 的根类 `.pane--list`（`data-testid="note-list"`）。 */
.workspace:has(> .workspace__reopen) :deep(.pane--list .pane-header) {
  padding-left: 44px;
}

/* 编辑器顶栏。
   原来只有 `gap: var(--sp-2)`（8px）+ `nowrap`，而这一栏里挤着
   「归属下拉 · 固定 · 删除」三件东西 —— 8px 间距让它们看着像**一串挤在一起的
   按钮**，而不是"这篇笔记的属性"（用户反馈"对不齐、层级乱"）。

   这里把「固定 / 删除」这类**危险/次要动作**用 `margin-left:auto` 推到右侧，
   让左侧的标题+归属占住视觉主线 —— 与 Notion/Obsidian 的编辑器顶栏一致：
   左边是这篇是什么，右边是对它做什么。 */
.editor-head {
  gap: var(--sp-2);
  flex-wrap: nowrap;
}

/* 顶栏右侧动作区：与左侧标题之间留出明确的分界。 */
.editor-head__actions {
  display: flex;
  align-items: center;
  gap: var(--sp-1);
  margin-left: auto;
  padding-left: var(--sp-2);
  flex: 0 0 auto;
}

.editor-head__folder {
  width: auto;
  max-width: 180px;
  min-height: var(--touch);
  font-size: var(--text-sm);
}

.editor-blank {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--sp-3);
  padding: var(--sp-8);
}
</style>
