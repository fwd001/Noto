/**
 * 快捷新建模板的门禁。
 *
 * 两条最要紧的判据不是"模板长什么样"，而是：
 *  ① **第一个块必须留空** —— 标题是从正文第一行推出来的（核心 `extract.rs:38`），
 *     模板占了第一行，用户起的标题就被模板文字顶掉，列表里看到的不是他的名字；
 *  ② 模板产出的 doc 必须能被我们自己的解析器原样读回来（`docToBlocks`）——
 *     不然它发给核心之后会变成"暂不支持的内容"那一格。
 * 再加一条端到端的：点 ▾ 选"待办清单"，真发出去的 `create_note` 载荷就是那个形状。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import App from './App.vue';
import { noteFixture, stubLocalService, type LocalService } from './testing/http';
import { NOTE_TEMPLATES, TEMPLATE_CHOICES, templateById } from './editor/templates';
import { docToBlocks, isValidBlockId } from './editor/model';

async function settle(times = 8): Promise<void> {
  for (let i = 0; i < times; i += 1) {
    await flushPromises();
  }
}

describe('模板表本身', () => {
  it('默认那颗不混进菜单：菜单里只剩非空白的模板', () => {
    expect(NOTE_TEMPLATES[0].id).toBe('blank');
    expect(TEMPLATE_CHOICES.map((t) => t.id)).toEqual(['todo', 'meeting']);
  });

  it('每个模板的第一个块都是空段落（标题留给用户）', () => {
    for (const tpl of NOTE_TEMPLATES) {
      const blocks = docToBlocks(tpl.build());
      expect(blocks[0].type, `${tpl.id} 的第一块不是段落`).toBe('paragraph');
      if (blocks[0].shape === 'text') {
        expect(blocks[0].content, `${tpl.id} 的第一块被模板文字占了`).toHaveLength(0);
      }
    }
  });

  it('块 id 合法且不重复（重复会被解析器重新发号，等于形状对不上）', () => {
    for (const tpl of NOTE_TEMPLATES) {
      const ids = tpl.build().content.map((b) => b.id ?? '');
      expect(ids.every((id) => isValidBlockId(id)), `${tpl.id} 有非法 id`).toBe(true);
      expect(new Set(ids).size).toBe(ids.length);
    }
  });

  it('待办 = 1 段 + 3 个未勾选的清单项；会议 = 带两行小标题的骨架', () => {
    const todo = docToBlocks(templateById('todo')!.build());
    expect(todo.map((b) => b.type)).toEqual(['paragraph', 'checklistItem', 'checklistItem', 'checklistItem']);
    for (const item of todo.slice(1)) {
      if (item.shape === 'text' && item.type === 'checklistItem') expect(item.attrs.checked).toBe(false);
    }

    const meeting = docToBlocks(templateById('meeting')!.build());
    const texts = meeting.map((b) => (b.shape === 'text' ? b.content.map((i) => i.text).join('') : ''));
    expect(texts).toContain('参会');
    expect(texts).toContain('结论');
    expect(meeting.map((b) => b.type)).toContain('checklistItem');
  });

  it('空白模板就是一片空：一个空段落，没有别的', () => {
    const blank = docToBlocks(templateById('blank')!.build());
    expect(blank).toHaveLength(1);
    expect(blank[0].type).toBe('paragraph');
  });
});

describe('列表头那颗 ▾', () => {
  let service: LocalService;

  beforeEach(() => {
    setActivePinia(createPinia());
    vi.useFakeTimers();
    vi.stubGlobal('EventSource', undefined);
    window.matchMedia = vi.fn().mockImplementation(() => ({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() }));
    service = stubLocalService({
      list_notes: () => [noteFixture({ id: 'note-1' })],
      list_folders: () => [{ id: 'folder-work', parentId: null, name: '工作', systemKind: null }],
      get_note: () => noteFixture({ id: 'note-1' }),
      create_note: (args) => noteFixture({ id: 'note-new', doc: args.doc }),
      account: () => null,
      stats: () => ({ notes: 1, notesInTrash: 0, folders: 1, attachments: 0, dbBytes: 4096 }),
      open_conflicts: () => [],
    });
  });

  afterEach(() => {
    vi.useRealTimers();
    document.body.innerHTML = '';
  });

  it('选"待办清单"发出去的 create_note 载荷就是那个骨架', async () => {
    const wrapper = mount(App, { attachTo: document.body });
    await settle();
    await wrapper.get('[data-testid="new-note-templates"]').trigger('click');
    await settle();
    await wrapper.get('[data-testid="template-todo"]').trigger('click');
    await settle();

    const calls = service.callsOf('create_note');
    expect(calls).toHaveLength(1);
    const doc = calls[0].args.doc as { content: Array<{ type: string }> };
    expect(doc.content.map((b) => b.type)).toEqual(['paragraph', 'checklistItem', 'checklistItem', 'checklistItem']);
  });

  it('主按钮那颗仍然直接建空白页，不弹菜单', async () => {
    const wrapper = mount(App, { attachTo: document.body });
    await settle();
    await wrapper.get('[data-testid="new-note"]').trigger('click');
    await settle();
    const calls = service.callsOf('create_note');
    expect(calls).toHaveLength(1);
    const doc = calls[0].args.doc as { content: Array<{ type: string }> };
    expect(doc.content).toHaveLength(1);
    expect(wrapper.find('[data-testid="app-popover-panel"]').exists(), '点主按钮不该把模板菜单弹出来').toBe(false);
  });
});
