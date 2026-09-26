/**
 * 附件这条边的契约测试。
 *
 * 为什么单独一份：`attach_file` 曾经前端发 `{noteId, blockId, role}`、核心还要
 * `localPath` + `mediaType` 两个必填字段 —— 点"插入图片/附件"必然 `bad_args`，
 * 而这条边一条测试都没有（账户那条边一模一样的故事）。这里盯的就是**形状**：
 * 平铺、camelCase、字段一个不多一个不少。
 */
import { describe, expect, it, vi } from 'vitest';
import {
  attachmentAttrs,
  AttachmentEmpty,
  MAX_ATTACHMENT_BYTES,
  readAsBase64,
  toAttachPayload,
  toDataUrl,
  AttachmentTooLarge,
} from './attachmentWire';

/** 伪装一个 File：大小可控，且"有没有被真读过字节"可观测。 */
function fakeFile(name: string, type: string, bytes: number[], size = bytes.length): File {
  const file = new File([new Uint8Array(bytes)], name, { type });
  if (size !== bytes.length) Object.defineProperty(file, 'size', { value: size });
  return file;
}

describe('attach_file 的线格式', () => {
  it('载荷是平铺的 camelCase，且恰好是核心认的那几个键', async () => {
    const payload = await toAttachPayload('note-1', 'block-1', 'inline', fakeFile('图.png', 'image/png', [1, 2, 3]));
    expect(Object.keys(payload).sort()).toEqual(
      ['blockId', 'bytesBase64', 'filename', 'mediaType', 'noteId', 'role'].sort(),
    );
    expect(payload.noteId).toBe('note-1');
    expect(payload.blockId).toBe('block-1');
    expect(payload.role).toBe('inline');
    expect(payload.filename).toBe('图.png');
    // 不许再包一层 `req`/`draft`：核心那边是平铺的，包了就是一整包退回默认值
    expect(JSON.stringify(payload)).not.toContain('"req"');
  });

  it('文件没有 type 时发空串，由核心决定落库形态', async () => {
    const payload = await toAttachPayload('n', 'b', 'file', fakeFile('无扩展名', '', [7]));
    expect(payload.mediaType).toBe('');
  });

  it('超限在**读字节之前**就拒，且带得出字节数', async () => {
    const file = fakeFile('大.png', 'image/png', [1, 2, 3], MAX_ATTACHMENT_BYTES + 1);
    const spy = vi.spyOn(file, 'arrayBuffer');
    await expect(toAttachPayload('n', 'b', 'inline', file)).rejects.toBeInstanceOf(AttachmentTooLarge);
    expect(spy).not.toHaveBeenCalled();
  });

  it('空文件在前端就拦掉（核心也会拒，但不该为此走一次 IPC）', async () => {
    await expect(toAttachPayload('n', 'b', 'inline', fakeFile('空.png', 'image/png', []))).rejects.toBeInstanceOf(AttachmentEmpty);
  });
});

describe('字节 → base64', () => {
  it('与 RFC 4648 的已知答案逐字符一致（含三种 padding 与 0/255 边界）', async () => {
    // 不引 Node 类型也不拿实现互校：这些是标准里的固定答案。
    const vectors: [number[], string][] = [
      [[], ''],
      [[1], 'AQ=='],
      [[1, 2], 'AQI='],
      [[1, 2, 3], 'AQID'],
      [[102, 111, 111], 'Zm9v'],
      [[102, 111, 111, 98], 'Zm9vYg=='],
      [[255], '/w=='],
      [[255, 255, 255], '////'],
      [[0], 'AA=='],
    ];
    for (const [bytes, want] of vectors) {
      const file = fakeFile('x.bin', 'application/octet-stream', bytes);
      expect(await readAsBase64(file)).toBe(want);
    }
  });

  it('1 MB 也不把栈撑爆，并且编码这步本身要够快（用户点一下不该卡住界面）', async () => {
    const big = new Uint8Array(1024 * 1024).fill(97);
    const file = new File([big], 'big.bin', { type: 'application/octet-stream' });
    const t0 = Date.now();
    const out = await readAsBase64(file);
    const tookMs = Date.now() - t0;
    expect(out.length).toBe(Math.ceil(big.length / 3) * 4);
    const back = atob(out);
    expect(back.length).toBe(big.length);
    expect(back.charCodeAt(0)).toBe(97);
    expect(back.charCodeAt(back.length - 1)).toBe(97);
    // 慢在这里就是"选完图界面卡住不动"。给一个宽松但不至于失效的界。
    expect(tookMs).toBeLessThan(1500);
  });
});

describe('结果 → 块属性', () => {
  it('核心回填了 sha 才写 sha256/ref，且 pending 一律落回 false', () => {
    const attrs = attachmentAttrs('inline', '图.png', { sha256: 'ab'.repeat(32), size: 3, mediaType: 'image/png' });
    expect(attrs.pending).toBe(false);
    expect(attrs.sha256).toBe('ab'.repeat(32));
    expect(attrs.ref).toBe(attrs.sha256);
    expect(attrs.name).toBe('图.png');
  });

  it('核心什么都没回时不许留半个附件键', () => {
    expect(attachmentAttrs('file', 'a.bin', null)).toEqual({ role: 'file', pending: false });
  });

  it('正文里不许出现 dataUrl —— 那等于把附件在 doc 里再存一份', () => {
    const attrs = attachmentAttrs('inline', '图.png', { sha256: 'ab'.repeat(32), size: 3, mediaType: 'image/png' });
    expect(Object.keys(attrs)).not.toContain('dataUrl');
    expect(JSON.stringify(attrs)).not.toContain('base64');
  });
});

describe('显示用的 data URL', () => {
  it('媒体类型照常带上，认不出来的一律按 octet-stream 而不是照抄', () => {
    expect(toDataUrl({ mediaType: 'image/png', bytesBase64: 'AAA=' })).toBe('data:image/png;base64,AAA=');
    expect(toDataUrl({ mediaType: 'javascript:alert(1)', bytesBase64: 'AAA=' })).toBe('data:application/octet-stream;base64,AAA=');
    expect(toDataUrl({ bytesBase64: 'AAA=' })).toBe('data:application/octet-stream;base64,AAA=');
  });

  it('没有字节就没有 URL（UI 显示占位而不是空 src）', () => {
    expect(toDataUrl(null)).toBeNull();
    expect(toDataUrl({ mediaType: 'image/png' })).toBeNull();
  });
});
