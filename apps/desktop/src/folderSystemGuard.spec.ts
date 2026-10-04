/**
 * 「默认本上那三颗点了必报错的按钮」的回归门禁。
 *
 * 真机事实（读代码 + 读载荷，不是推测）：核心 `assert_folder_writable`
 * （`crates/notera-store/src/store.rs:996`）对 `system_kind` 非空的文件夹一律拒绝
 * 改名 / 移动 / 删除，`delete_folder` 里还有一条更早的显式守卫（同文件 :644）。
 * 而 `FolderDto` **一直**把 `systemKind` 发出来 —— 是前端 `Folder` 类型没声明这一格、
 * `ensureNode` 也没往下带，所以界面无从区分，给"默认"那一行照原样画了三颗按钮。
 * 用户点下去得到的是一句 Constraint 错：典型的"控件在、后果没有"。
 *
 * 判据分两层，缺一不可：
 *   ① 读侧：核心载荷里的 `systemKind` 必须活着走到节点对象上（它坏了，② 就是假绿）；
 *   ② 渲染侧：默认本那行不许出现这三颗，普通文件夹三颗都必须在（反向腿，
 *      防止有人用"整条工具栏都不画"糊过去）。
 */
import { beforeEach, describe, expect, it } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import FolderTree from './components/FolderTree.vue';
import { buildTree, useFolderStore } from './stores/folders';

/** 形状照核心 `FolderDto`（camelCase 线格式），包含前端此前丢掉的那一格。 */
const CORE_FOLDERS = [
  {
    id: 'folder-default',
    parentId: null,
    name: '默认',
    color: null,
    systemKind: 'default',
    noteCount: 2,
    sortOrder: 0,
    children: [],
  },
  {
    id: 'folder-work',
    parentId: null,
    name: '工作',
    color: null,
    systemKind: null,
    noteCount: 0,
    sortOrder: 1,
    children: [],
  },
];

function nodeOf(id: string) {
  return useFolderStore().nodes.find((n) => n.id === id);
}

beforeEach(() => {
  setActivePinia(createPinia());
});

describe('读侧：systemKind 必须活着走到节点上', () => {
  it('核心给的 systemKind 不被 ensureNode 丢掉', () => {
    const tree = buildTree(CORE_FOLDERS);
    expect(tree.find((n) => n.id === 'folder-default')?.systemKind).toBe('default');
    expect(tree.find((n) => n.id === 'folder-work')?.systemKind).toBeNull();
  });

  it('载荷缺这一格时按"普通文件夹"处理，不许凭名字猜默认本', () => {
    // "默认"只是用户可改的显示名；拿名字当角色判据，改名就把守卫绕过了。
    const tree = buildTree([{ ...CORE_FOLDERS[0], systemKind: undefined, name: '我的收件箱' }]);
    expect(tree[0].systemKind).toBeNull();
  });
});

describe('渲染侧：按钮可见性跟着核心的判据走', () => {
  async function render() {
    const folders = useFolderStore();
    folders.nodes = buildTree(CORE_FOLDERS);
    const wrapper = mount(FolderTree);
    await flushPromises();
    return wrapper;
  }

  it('默认本那一行没有改名/删除；普通文件夹两颗都在', async () => {
    const wrapper = await render();
    expect(nodeOf('folder-default')?.systemKind, '夹具没生效：读不到 systemKind').toBe('default');

    // 计数而不是"存在性"：两行里只允许普通文件夹那一行贡献这两颗。
    expect(wrapper.findAll('[data-testid="folder-rename"]')).toHaveLength(1);
    expect(wrapper.findAll('[data-testid="folder-delete"]')).toHaveLength(1);

    // 反向腿：普通文件夹的两颗不能被一起藏掉。
    const workRow = wrapper.get('[data-testid="folder-folder-work"]').element.closest('.tree__row');
    expect(workRow?.querySelector('[data-testid="folder-delete"]'), '"工作"那行该有删除').not.toBeNull();
    expect(workRow?.querySelector('[data-testid="folder-rename"]'), '"工作"那行该有改名').not.toBeNull();
  });

  it('层级 UI 已经拿掉：没有"新建子文件夹"、也没有"移动到父级"', async () => {
    const wrapper = await render();
    expect(wrapper.find('[data-testid^="folder-new-sub-"]').exists(), '还留着"在这下面新建"').toBe(false);
    expect(wrapper.find('[data-testid="folder-move"]').exists(), '还留着"移动到父级"').toBe(false);
  });

  it('所有文件夹行同一层：缩进不许随层级变', async () => {
    const wrapper = await render();
    const pads = wrapper.findAll('[data-testid="folder-row"]').map((r) => getComputedStyle(r.element).paddingLeft);
    expect(pads.length, '一行都没量到，判据在空转').toBeGreaterThan(0);
    expect(new Set(pads).size, `缩进不止一种：${JSON.stringify(pads)}`).toBe(1);
  });

  it('藏掉按钮不等于只藏图标：改名输入框也进不去', async () => {
    const wrapper = await render();
    const defaultRow = wrapper.get('[data-testid="folder-folder-default"]').element.closest('.tree__row');
    expect(defaultRow?.querySelector('[data-testid="folder-delete"]'), '默认本不该有删除入口').toBeNull();
    // 点了也没有确认层可弹（确认层由 confirmingDelete 驱动，而它只能由那颗按钮置上）。
    expect(wrapper.find('[data-testid="folder-delete-confirm"]').exists()).toBe(false);
  });
});
