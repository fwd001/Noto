/**
 * 文件夹树规范化。这里刻意用**真桥原样吐出来的 JSON**（不是手搓的理想形状）：
 * /cmd/list_folders 下发的是嵌套树，而曾经只有平铺输入被测过 —— 于是测试全绿、
 * 产品里子文件夹却整个隐形（侧栏、"移动到"下拉、导出选择器都只剩根）。
 */
import { beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { stubLocalService, stubServiceDown } from '../testing/http';
import { buildTree, useFolderStore } from './folders';

/** 形状与键名照抄 dev 桥响应，含 null 的 color / systemKind。 */
function treeNode(id: string, parentId: string | null, name: string, children: unknown[] = []): Record<string, unknown> {
  return { children, color: null, id, name, noteCount: 0, parentId, systemKind: null };
}

beforeEach(() => {
  setActivePinia(createPinia());
});

describe('后端给的嵌套树', () => {
  it('子层一个都不许丢，顺序按深度优先', async () => {
    const tree = [
      treeNode('root', null, '默认本', [
        treeNode('a', 'root', '项目', [treeNode('b', 'a', '细节')]),
        treeNode('c', 'root', '会议'),
      ]),
    ];
    stubLocalService({ list_folders: () => tree });
    const folders = useFolderStore();
    await folders.load();
    // 同级按名称排序（zh 校对），所以这里钉的是"谁在、第几层、路径是什么"，不是 DFS 顺序
    expect(folders.flat.map((entry) => `${entry.depth}:${entry.node.name}`).sort()).toEqual(
      ['0:默认本', '1:会议', '1:项目', '2:细节'].sort(),
    );
    expect(folders.byId.get('b')?.path.join(' / ')).toBe('默认本 / 项目 / 细节');
    expect(folders.byId.get('c')?.path.join(' / ')).toBe('默认本 / 会议');
  });

  it('同一棵树不论先给父还是先给孙都能拼好', () => {
    const parent = treeNode('a', null, '父', [treeNode('b', 'a', '子')]);
    expect(buildTree([parent]).flatMap((r) => (r.children ?? []).map((c) => c.name))).toEqual(['子']);
    expect(buildTree([treeNode('b', 'a', '子'), parent]).flatMap((r) => (r.children ?? []).map((c) => c.name))).toEqual(['子']);
  });

  it('自环不许把树撑爆', () => {
    const loop = treeNode('x', 'x', '自环');
    const built = buildTree([loop]);
    expect(built.map((n) => n.id)).toEqual(['x']);
  });

  /**
   * 0.0.66（缺口 G63）起这是一格**可达状态**：核心交给界面的那一排不再含回收站里的文件夹，
   * 而对端把某个父级打成墓碑时不会替本机重新挂子层（`apply` 只写 `deleted_at`）。
   * 于是"父不在这一排里、子还在"必须顶到最上面 —— 静默丢掉就是"文件夹连同里面的笔记一起消失"。
   */
  it('父不在这一排里（被回收站滤掉）时，子层顶到最上面而不是消失', async () => {
    stubLocalService({ list_folders: () => [treeNode('root', null, '默认本'), treeNode('kid', 'gone', '无父的子层')] });
    const folders = useFolderStore();
    await folders.load();
    expect(folders.flat.map((entry) => `${entry.depth}:${entry.node.name}`).sort()).toEqual(
      ['0:无父的子层', '0:默认本'].sort(),
    );
  });
});

/**
 * §6-11「颜色」的调用边（第 39 刀）。
 *
 * 这里钉的是**这次调用发没发、发的形状对不对**，不是核心那一位列存没存下（后者由
 * `crates/notera-host/tests/folder_color.rs` 拿真序列化输出验）。绿单测掩盖坏调用边是
 * 本仓库反复栽过的一族：色点画得出来，前提是界面把 `set_folder_color` 按这个名字、
 * 这两个键发出去。
 */
describe('给文件夹上色', () => {
  it('按命令名与两个键发出去，且树上那一行立刻跟着变', async () => {
    const svc = stubLocalService({
      list_folders: () => [treeNode('root', null, '默认本', [treeNode('a', 'root', '项目')])],
      set_folder_color: () => ({ ...treeNode('a', 'root', '项目'), color: '#c2410c' }),
    });
    const folders = useFolderStore();
    await folders.load();
    await folders.setColor('a', '#c2410c');
    expect(svc.callsOf('set_folder_color').length, '一次都没发命令：色点是画上去的假象').toBe(1);
    expect(svc.lastArgsOf('set_folder_color')).toEqual({ color: '#c2410c', id: 'a' });
    expect(folders.byId.get('a')?.node.color).toBe('#c2410c');
  });

  it('清掉颜色发的是 null，不是空串', async () => {
    const svc = stubLocalService({
      list_folders: () => [treeNode('a', null, '项目')],
      set_folder_color: () => treeNode('a', null, '项目'),
    });
    const folders = useFolderStore();
    await folders.load();
    await folders.setColor('a', null);
    expect(svc.lastArgsOf('set_folder_color')).toEqual({ color: null, id: 'a' });
    expect(folders.byId.get('a')?.node.color ?? null).toBe(null);
  });

  /** 失败时不许先把点抹掉再报错：那会让用户以为是自己点错了，而且下一次同步会拿这个假状态去比。 */
  it('命令没成（本地服务不在）时那一行保持原色，并把错留给界面说', async () => {
    stubLocalService({ list_folders: () => [{ ...treeNode('a', null, '项目'), color: '#1e40af' }] });
    const folders = useFolderStore();
    await folders.load();
    expect(folders.byId.get('a')?.node.color).toBe('#1e40af');
    stubServiceDown();
    await folders.setColor('a', '#c2410c');
    expect(folders.byId.get('a')?.node.color, '发失败了却已经把颜色改掉 —— 屏幕说的是假话').toBe('#1e40af');
    expect(typeof folders.errorKey === 'string' && folders.errorKey.length > 0, '失败没留下任何可显示的错误键').toBe(true);
  });
});
