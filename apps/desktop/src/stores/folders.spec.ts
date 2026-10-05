/**
 * 文件夹树规范化。这里刻意用**真桥原样吐出来的 JSON**（不是手搓的理想形状）：
 * /cmd/list_folders 下发的是嵌套树，而曾经只有平铺输入被测过 —— 于是测试全绿、
 * 产品里子文件夹却整个隐形（侧栏、"移动到"下拉、导出选择器都只剩根）。
 */
import { beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { stubLocalService } from '../testing/http';
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
