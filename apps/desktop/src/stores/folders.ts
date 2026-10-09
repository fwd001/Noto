/** 文件夹树：新建 / 重命名 / 移动 / 删除（删除不级联删笔记，由后端保证）。 */
import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { callCommand } from '../api/bridge';
import { Commands, type Folder, type FolderNode } from '../api/types';
import { asBridgeError } from '../util/errors';

export interface FlatFolder {
  node: FolderNode;
  depth: number;
  path: string[];
}

function ensureNode(value: unknown): FolderNode | null {
  if (typeof value !== 'object' || value === null) return null;
  const raw = value as Record<string, unknown> & Partial<FolderNode>;
  if (typeof raw.id !== 'string' || raw.id.length === 0) return null;
  return {
    id: raw.id,
    parentId: typeof raw.parentId === 'string' ? raw.parentId : null,
    name: typeof raw.name === 'string' && raw.name.length > 0 ? raw.name : '未命名',
    children: Array.isArray(raw.children) ? raw.children.filter((c): c is FolderNode => typeof c === 'object' && c !== null) : [],
    ...(typeof raw.noteCount === 'number' ? { noteCount: raw.noteCount } : {}),
    ...(typeof raw.sortOrder === 'number' ? { sortOrder: raw.sortOrder } : {}),
    ...(typeof raw.color === 'string' ? { color: raw.color } : {}),
    // 这一格必须由核心决定，不能在这里凭名字猜："默认"是用户可改的显示名，
    // 而 systemKind 才是角色。丢了它，默认本上就会长出三颗点了必报错的按钮。
    systemKind: typeof raw.systemKind === 'string' ? raw.systemKind : null,
  };
}

/** 后端可能给树（FolderNode[]）也可能给平铺（Folder[]）；两种都能用。 */
export function buildTree(value: unknown): FolderNode[] {
  const list = Array.isArray(value) ? value : [];
  // 嵌套树里的子层只存在于 `children` 中，顶层数组看不到 —— 所以先把整片森林
  // 收进同一个池子，再统一按 parentId 重建。曾经这里在收集前就把 children 清空了，
  // 于是"后端给的树"被削成只剩根，侧栏 / "移动到" / 导出选择器一起失去子文件夹。
  const pool = new Map<string, FolderNode>();
  const collect = (items: readonly unknown[]): void => {
    for (const item of items) {
      const node = ensureNode(item);
      if (!node || pool.has(node.id)) continue;
      const nested = node.children;
      pool.set(node.id, node);
      collect(nested);
    }
  };
  collect(list);
  const nodes = [...pool.values()];
  if (nodes.length === 0) return [];
  for (const node of nodes) node.children = [];
  const roots: FolderNode[] = [];
  for (const node of nodes) {
    const parent = node.parentId ? pool.get(node.parentId) : undefined;
    if (parent && parent.id !== node.id) parent.children = [...(parent.children ?? []), node];
    else roots.push(node);
  }
  const sortRecursively = (list2: FolderNode[]) => {
    list2.sort((a, b) => (a.sortOrder ?? 0) - (b.sortOrder ?? 0) || a.name.localeCompare(b.name, 'zh-CN'));
    for (const child of list2) sortRecursively(child.children ?? []);
  };
  sortRecursively(roots);
  return roots;
}

export function flattenTree(roots: readonly FolderNode[], limit = 64): FlatFolder[] {
  const out: FlatFolder[] = [];
  const walk = (nodes: readonly FolderNode[], depth: number, path: string[]) => {
    for (const node of nodes) {
      if (out.length >= limit) return;
      out.push({ node, depth, path: [...path, node.name] });
      walk(node.children ?? [], depth + 1, [...path, node.name]);
    }
  };
  walk(roots, 0, []);
  return out;
}

export const useFolderStore = defineStore('folders', () => {
  const nodes = ref<FolderNode[]>([]);
  const loading = ref(false);
  const errorKey = ref<string | null>(null);
  const flat = computed<FlatFolder[]>(() => flattenTree(nodes.value));
  const byId = computed(() => new Map(flat.value.map((entry) => [entry.node.id, entry])));
  /**
   * 「默认本」那一格（核心 bootstrap 出来的 `system_kind='default'`）：
   * 它是产品里**唯一**的"还没整理"去处 —— 笔记必须属于某个文件夹（`list_notes` 内连接 folders，
   * `create_note` / `set_note_folder` 的入参都是非空 uuid）。界面上两处要认它：
   * 编辑器顶栏「移到」的落点，与删除文件夹那条确认文案里说的去处。
   */
  const defaultNode = computed(() => flat.value.find((entry) => entry.node.systemKind === 'default') ?? null);

  async function load(): Promise<void> {
    loading.value = true;
    errorKey.value = null;
    try {
      nodes.value = buildTree(await callCommand<unknown>(Commands.listFolders, {}));
    } catch (error) {
      errorKey.value = asBridgeError(error).messageKey;
    } finally {
      loading.value = false;
    }
  }

  function replaceNode(node: FolderNode | null): void {
    if (!node) return;
    const walk = (list: FolderNode[]): FolderNode[] =>
      list.map((item) => (item.id === node.id ? { ...item, ...node, children: item.children } : { ...item, children: walk(item.children ?? []) }));
    const updated = walk(nodes.value);
    const stillThere = flattenTree(updated).some((entry) => entry.node.id === node.id);
    nodes.value = stillThere ? updated : buildTree([...flattenTree(updated).map((entry) => entry.node), node]);
  }

  async function create(parentId: string | null, name: string): Promise<Folder | null> {
    try {
      const created = await callCommand<Folder>(Commands.createFolder, { parentId, name: name.trim() });
      await load();
      return created;
    } catch (error) {
      errorKey.value = asBridgeError(error).messageKey;
      return null;
    }
  }

  async function rename(id: string, name: string): Promise<void> {
    try {
      replaceNode((await callCommand<Folder>(Commands.renameFolder, { id, name: name.trim() })) as unknown as FolderNode);
      await load();
    } catch (error) {
      errorKey.value = asBridgeError(error).messageKey;
    }
  }

  async function remove(id: string): Promise<void> {
    try {
      await callCommand<null>(Commands.deleteFolder, { id });
      await load();
    } catch (error) {
      errorKey.value = asBridgeError(error).messageKey;
    }
  }

  function nameOf(id: string | null | undefined): string {
    if (!id) return '';
    return byId.value.get(id)?.node.name ?? '';
  }

  async function setColor(id: string, color: string | null): Promise<void> {
    try {
      replaceNode((await callCommand<Folder>(Commands.setFolderColor, { id, color })) as unknown as FolderNode);
    } catch (error) {
      errorKey.value = asBridgeError(error).messageKey;
    }
  }

  return { nodes, flat, byId, defaultNode, loading, errorKey, load, create, rename, setColor, remove, nameOf };
});
