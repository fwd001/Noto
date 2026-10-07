/**
 * 拖文件进窗口（§6「能力位已开、核心接口已接受本机路径，只差一个 drop 处理器」那一格）。
 *
 * 这里盯两件"两条入口必须说同一件事"的东西：
 * ① 分流规则只有一处（`roleForFile`）—— 拖进来的和选文件进来的如果是各写一遍 `startsWith`，
 *    下一次改图片判定只会改到一半；
 * ② 组件那侧的接线形状（有 drop 处理器、真的 `preventDefault`、提示只在拖入时存在且不吃饭似地挡住指针）。
 * 真拖一次的行为由 `verify-layout` 第 ㊴ 腿在真浏览器 + 真核心上量。
 */
import { describe, expect, it } from 'vitest';
import { roleForFile } from './editor/attachmentWire';

const SRC = import.meta.glob<string>('./components/RichEditor.vue', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

const src = Object.values(SRC)[0] ?? '';

describe('拖入文件的分流', () => {
  it('图片进正文，其余进附件行', () => {
    expect(roleForFile({ type: 'image/png' })).toBe('inline');
    expect(roleForFile({ type: 'image/svg+xml' })).toBe('inline');
    expect(roleForFile({ type: 'application/pdf' })).toBe('file');
    expect(roleForFile({ type: '' })).toBe('file');
  });

  it('判的是 MIME 前缀，不是文件名（"报告.image.pdf" 不该进正文）', () => {
    expect(roleForFile({ type: 'application/pdf' })).toBe('file');
    expect(roleForFile({ type: 'image/heic' })).toBe('inline');
  });
});

describe('RichEditor 的接线形状', () => {
  it('三个事件都接上了，且用的是同一个"这一栏"的容器', () => {
    expect(src).toContain('@dragover="onDragOver"');
    expect(src).toContain('@dragleave="onDragLeave"');
    expect(src).toContain('@drop="onDrop"');
  });

  it('真的 preventDefault（不拦浏览器会自己去打开那个文件）', () => {
    expect(src).toMatch(/function onDragOver[\s\S]{0,600}?event\.preventDefault\(\)/);
    expect(src).toMatch(/function onDrop[\s\S]{0,600}?event\.preventDefault\(\)/);
  });

  it('分流走的是那一处判据，不在组件里再写一遍 startsWith', () => {
    expect(src).toContain('roleForFile(');
    expect(/attachFile\(\s*file\.type\.startsWith/.test(src)).toBe(false);
  });

  it('提示只在真的拖着东西时存在，且不挡住指针（挡住了就松不了手）', () => {
    expect(src).toContain('v-if="dropping"');
    expect(src).toMatch(/\.editor-drop\s*\{[\s\S]{0,600}?pointer-events:\s*none/);
  });

  it('不能写的时候要按同一份说法拒绝（复用正文那条横幅的三个键）', () => {
    expect(src).toMatch(/DROP_BLOCKED_KEYS[\s\S]{0,300}?editor\.libraryReadOnly/);
    expect(src).toMatch(/DROP_BLOCKED_KEYS[\s\S]{0,300}?editor\.versionTooNew/);
    expect(src).toMatch(/DROP_BLOCKED_KEYS[\s\S]{0,300}?state\.inTrash/);
    expect(src).toMatch(/function onDrop[\s\S]{0,600}?store\.writeBlocked/);
  });
});
