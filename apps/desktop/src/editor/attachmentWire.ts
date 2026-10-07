/**
 * 附件这条边的**唯一**翻译处：浏览器 `File` ⇄ 命令面的平铺 JSON。
 *
 * 为什么单独一个文件：`attach_file` 曾经前端发 `{noteId, blockId, role}`、
 * 核心要的是 `{noteId, blockId, role, localPath, mediaType}` —— 少两个必填字段，
 * 于是点"插入图片/附件"必然 `bad_args`，而这条边没有任何测试盯着它（账户那条边
 * 一模一样的故事，见 `sync/accountWire.ts`）。形状只写一遍，测试也只盯这一处。
 *
 * 还有一条铁律在这里落地：**字节可以经过前端，但"存成什么"永远由核心决定** ——
 * sha256 由核心算、blob 由核心落盘、`attachments`/`note_attachments` 由核心写。
 * 前端这里只做两件事：把 File 变成 base64，以及把结果变成块上的属性。
 */

/** 与核心 `MAX_ATTACHMENT_BYTES` 同一个数：单个附件的上限（SYNC-PROTOCOL §13 的一轮预算是 ≤4 文件 / ≤64 MiB）。 */
export const MAX_ATTACHMENT_BYTES = 32 * 1024 * 1024;

/** 发给 `attach_file` 的载荷：平铺、camelCase，没有 `req` 之类的包装。 */
export interface AttachPayload {
  noteId: string;
  blockId: string;
  role: 'inline' | 'file';
  mediaType: string;
  filename: string | null;
  bytesBase64: string;
}

/** 超限时抛这个，UI 按 messageKey 出文案 —— 不把原始字节数塞进提示里。 */
export class AttachmentTooLarge extends Error {
  constructor(public readonly bytes: number) {
    super(`attachment too large: ${bytes}`);
    this.name = 'AttachmentTooLarge';
  }
}

/** 空文件不值得挂上去（核心也会拒），先在前端拦掉，省一次 IPC 和一个假附件块。 */
export class AttachmentEmpty extends Error {
  constructor() {
    super('attachment is empty');
    this.name = 'AttachmentEmpty';
  }
}

/**
 * 拖进来的文件按类型分流：图片进正文，其余进附件行 —— 与工具条那两颗按钮同一套语义。
 * 单独一个函数是为了让"拖放"和"选文件"两条入口共用同一个判据，而不是各写一遍 `startsWith`。
 */
export function roleForFile(file: { type: string }): 'inline' | 'file' {
  return file.type.startsWith('image/') ? 'inline' : 'file';
}

/**
 * File → base64。
 *
 * 分块 `btoa`：`String.fromCharCode(...bytes)` 在几 MB 上会直接把栈撑爆，
 * 而"选一张大图就崩"是用户最先撞到的那类崩溃。
 */
export async function readAsBase64(file: File): Promise<string> {
  const bytes = new Uint8Array(await file.arrayBuffer());
  const chunk = 0x8000;
  let binary = '';
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

/**
 * 组装载荷。**先判大小再读字节**：40 MB 的文件不该被整个读进内存之后才说"太大"。
 */
export async function toAttachPayload(
  noteId: string,
  blockId: string,
  role: 'inline' | 'file',
  file: File,
): Promise<AttachPayload> {
  if (file.size > MAX_ATTACHMENT_BYTES) throw new AttachmentTooLarge(file.size);
  if (file.size === 0) throw new AttachmentEmpty();
  return {
    noteId,
    blockId,
    role,
    // 浏览器对没有扩展名的文件给不出 type；空串交给核心按 octet-stream 落库，
    // 不在这里替核心做决定。
    mediaType: file.type ?? '',
    filename: file.name || null,
    bytesBase64: await readAsBase64(file),
  };
}

/** `attach_file` 的返回 → 块上的属性。 */
export interface AttachmentResult {
  sha256?: string;
  size?: number;
  mediaType?: string;
  /** 笔记被这次写入推进后的新 rev，编辑器必须接住（见 stores/editor.ts 的 attachFile）。 */
  rev?: number;
}

export function attachmentAttrs(role: 'inline' | 'file', filename: string, out: AttachmentResult | null | undefined): Record<string, unknown> {
  const attrs: Record<string, unknown> = { role, pending: false };
  if (!out?.sha256) return attrs;
  attrs.sha256 = out.sha256;
  attrs.ref = out.sha256;
  if (typeof out.size === 'number') attrs.size = out.size;
  if (out.mediaType) attrs.mediaType = out.mediaType;
  if (filename) attrs.name = filename;
  return attrs;
}

/**
 * `attachment_data` 的返回 → 能塞进 `src` 的 data URL。
 *
 * 注意：**不能把它写进块属性**。块属性会进 doc、doc 会进同步载荷 —— 那等于每个附件
 * 在正文里再存一份 base64（体积膨胀 ~4/3，还会让每次编辑都拖着它）。所以显示用的
 * data URL 只活在内存里的那张表（`stores/editor.ts` 的 `attachmentUrls`）。
 */
export interface AttachmentData {
  sha256?: string;
  mediaType?: string;
  size?: number;
  bytesBase64?: string;
}

export function toDataUrl(data: AttachmentData | null | undefined): string | null {
  if (!data?.bytesBase64) return null;
  const media = /^image\/|^(application|text)\/|^audio\/|^video\//i.test(data.mediaType ?? '') ? data.mediaType : 'application/octet-stream';
  return `data:${media};base64,${data.bytesBase64}`;
}
