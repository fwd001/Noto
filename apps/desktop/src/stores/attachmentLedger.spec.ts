import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import type { Note } from '../api/types';
import { noteFixture, stubLocalService } from '../testing/http';
import { useEditorStore } from './editor';

/**
 * G74：打开一篇笔记时，界面必须知道"这篇引用的每个对象，本机到底有没有可用字节"。
 *
 * 此前的形状：只有图片那一支靠"取字节失败 ⇒ 没有 URL ⇒ 画占位"这个**副作用**表达了缺，
 * 而文件附件那颗芯片只认本次会话上传过的对象 ⇒ 从别的设备同步来、本机还没下载的附件
 * 画起来跟完好的一样，既不说明缺、也不给那两颗已存在的动作。
 *
 * 判据全部打在**这次调用发没发、落账收成了什么**上（见 [[verify-the-call-edge-not-just-the-callees-tests]]），
 * 而不是"占位画了没" —— 后者在 jsdom 里量不到。
 */
function noteWith(shaImg: string, shaAtt: string): Note {
  return noteFixture({
    doc: {
      v: 1,
      content: [
        { id: 'img00001', type: 'image', attrs: { sha256: shaImg } },
        { id: 'att00001', type: 'attachment', attrs: { sha256: shaAtt, name: '报告.pdf' } },
        { id: 'att00002', type: 'attachment', attrs: { sha256: shaAtt, name: '重复的一份' } },
      ],
    },
  }) as unknown as Note;
}

async function settle(times = 6): Promise<void> {
  for (let i = 0; i < times; i += 1) await Promise.resolve();
}

beforeEach(() => {
  setActivePinia(createPinia());
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('打开笔记时的附件读账', () => {
  it('两种块都要问到，且重复的 sha 只问一次', async () => {
    const service = stubLocalService({
      attachment_states: () => [
        { sha256: 'sha-img', localState: 'available', remoteState: 'present' },
        { sha256: 'sha-att', localState: 'missing', remoteState: 'present' },
      ],
    });
    useEditorStore().hydrate(noteWith('sha-img', 'sha-att'));
    await settle();

    const args = service.lastArgsOf('attachment_states') as { shas: string[] } | undefined;
    expect(args, '一次都没朝核心问账 ⇒ 附件那颗芯片永远只认识本次会话').toBeDefined();
    expect([...(args?.shas ?? [])].sort()).toEqual(['sha-att', 'sha-img']);
    expect(service.callsOf('attachment_states')).toHaveLength(1);
  });

  it('拿到回音之前不许当成"缺"—— 未知就是未知', async () => {
    const editor = useEditorStore();
    // 桩照核心那句契约来：**问过的每个 sha 都要有回音**（Rust 侧同一条由
    // `attachment_states_answers_every_asked_sha_with_wire_camel_case_keys` 钉着）。
    stubLocalService({
      attachment_states: () => [
        { sha256: 'sha-att', localState: 'missing', remoteState: 'present' },
        { sha256: 'sha-img', localState: 'available', remoteState: 'present' },
      ],
    });
    expect(editor.attachmentLedger('sha-att')).toBeNull();

    editor.hydrate(noteWith('sha-img', 'sha-att'));
    await settle();
    // 回的是核心那一行的原样（含 sha256）：界面不裁剪、不重新解释那对状态位。
    expect(editor.attachmentLedger('sha-att')).toEqual({ sha256: 'sha-att', localState: 'missing', remoteState: 'present' });
    expect(editor.attachmentLedger('sha-img')).toEqual({ sha256: 'sha-img', localState: 'available', remoteState: 'present' });
  });

  it('问账失败不许把附件说成缺，也不许拖累正文（INV：附件不阻塞文本）', async () => {
    const editor = useEditorStore();
    stubLocalService({});
    editor.hydrate(noteWith('sha-img', 'sha-att'));
    await settle();

    expect(editor.attachmentLedger('sha-att')).toBeNull();
    expect(editor.blocks.length).toBe(3);
    expect(editor.saveState).not.toBe('error');
  });
});
