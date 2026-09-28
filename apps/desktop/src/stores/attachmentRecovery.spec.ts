/**
 * 坏图/坏附件占位上那两个用户动作的**契约**测试（「重试取回」/「重新上传本机这份」）。
 *
 * 为什么单独一份、且只测到命令边界为止：这两个按钮的价值全在"意图递到核心、核心的话显示出来"。
 * 本仓在这类边上踩过两次同一形状的坑 —— `stats` 的键名漂移与 `attach_file` 的载荷包层，
 * 两次都是 TS 类型与前端 mock 一起绿灯、功能却是坏的。所以这里断言的是
 * **命令名字面量 + 载荷形状 + 返回体字段名 + 失败时显示的是翻译后的文案**，
 * 而不是"组件渲染出一颗按钮"（那由浏览器 lane 用真点击去证）。
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { stubLocalService } from '../testing/http';
import { useEditorStore } from './editor';
import { useToastStore } from './toasts';

const SHA = 'e'.repeat(64);

beforeEach(() => {
  setActivePinia(createPinia());
  vi.useRealTimers();
});

describe('「重试取回」', () => {
  it('发的命令名与载荷逐字对齐核心，返回体就是账上那对状态', async () => {
    const service = stubLocalService({
      attachment_retry: () => ({ sha256: SHA, localState: 'missing', remoteState: 'unknown' }),
      sync_now: () => ({}),
    });
    const editor = useEditorStore();

    const st = await editor.retryAttachmentFetch(SHA);

    expect(service.callsOf('attachment_retry')).toHaveLength(1);
    expect(service.lastArgsOf('attachment_retry')).toEqual({ sha256: SHA });
    expect(st).toEqual({ sha256: SHA, localState: 'missing', remoteState: 'unknown' });
    // 撤完该催一轮，否则用户要等到下一次 20 s 心跳才看到动静
    expect(service.callsOf('sync_now').length).toBeGreaterThanOrEqual(1);
  });

  it('核心说不必重试时，显示的是中文文案而不是漏出键名', async () => {
    stubLocalService({
      attachment_retry: () => ({
        ok: false,
        error: { code: 'nothing_to_retry', messageKey: 'cmd.nothing_to_retry', retryable: false },
      }),
      sync_now: () => ({}),
    });
    const editor = useEditorStore();
    const toasts = useToastStore();

    const st = await editor.retryAttachmentFetch(SHA);

    expect(st).toBeNull();
    const last = toasts.items.at(-1);
    expect(last?.level).toBe('error');
    expect(last?.text).toContain('不需要取回');
    // 漏出 `cmd.nothing_to_retry` 就等于把内部码当用户语言（§26 那条同一味道）
    expect(last?.text).not.toContain('cmd.');
  });

  it('块上没有 sha 时一条命令都不许发', async () => {
    const service = stubLocalService({ attachment_retry: () => ({}), sync_now: () => ({}) });
    const editor = useEditorStore();

    expect(await editor.retryAttachmentFetch(undefined)).toBeNull();
    expect(service.calls).toEqual([]);
  });
});

describe('「重新上传本机这份」', () => {
  it('递的是意图：命令名 attachment_reupload，载荷只有 sha256', async () => {
    const service = stubLocalService({
      attachment_reupload: () => ({ sha256: SHA, localState: 'available', remoteState: 'error' }),
      sync_now: () => ({}),
    });
    const editor = useEditorStore();

    const st = await editor.reuploadAttachment(SHA);

    expect(service.callsOf('attachment_reupload')).toHaveLength(1);
    expect(Object.keys(service.lastArgsOf('attachment_reupload') ?? {})).toEqual(['sha256']);
    // 核心把意图落到账上（error = 这一份被内容比对否定过），界面只回显
    expect(st?.remoteState).toBe('error');
    // 数组的 toContain 比的是**整项相等**，用它查"某条提示里含这句话"会永远查不到 ——
    // 这里要的是子串，判据写成 some(includes)，别让它变成一条松掉的断言。
    expect(toastTexts().some((line) => line.includes('覆盖上去'))).toBe(true);
  });

  it('本机那份不可用时把核心的那句原话翻译给用户，且不谎报成功', async () => {
    stubLocalService({
      attachment_reupload: () => ({
        ok: false,
        error: { code: 'nothing_to_upload', messageKey: 'cmd.nothing_to_upload', retryable: false },
      }),
      sync_now: () => ({}),
    });
    const editor = useEditorStore();

    expect(await editor.reuploadAttachment(SHA)).toBeNull();
    expect(toastTexts().some((line) => line.includes('不能上传'))).toBe(true);
  });
});

function toastTexts(): string[] {
  return useToastStore().items.map((item) => item.text);
}
