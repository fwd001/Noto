/**
 * 后端契约 DTO（唯一事实源：docs/diagram/architecture.html 的 ui / host / store 模块表）。
 * UI 层只消费这里声明的结构，不自行发明字段。
 */

export type Uuid = string;
export type Rfc3339 = string;
export type SafeHtml = string;

/* ------------------------------------------------------------------ 富文本 */

export interface Mark {
  kind: string;
  attrs?: Record<string, unknown>;
}

export interface Inline {
  text: string;
  marks?: Mark[];
}

export interface Block {
  id: string;
  type: string;
  attrs?: Record<string, unknown>;
  content?: Inline[];
  /** 后端标记的未知块（前向兼容容器） */
  unknown?: boolean;
  /**
   * 线格式必须容忍未知顶层字段（不变式 I7）：新版本客户端写入的字段，
   * 旧版本要原样带回，否则一次保存就把新内容抹掉。
   * 编辑器内部模型 EditorBlock 仍是严格类型，宽松只发生在这一层。
   */
  [extra: string]: unknown;
}

export interface NoteDoc {
  v: number;
  content: Block[];
}

/* -------------------------------------------------------------------- 笔记 */

export interface Note {
  id: Uuid;
  folderId: Uuid | null;
  doc: NoteDoc;
  docFormat?: number;
  title: string;
  plainText?: string;
  summary?: string;
  charCount?: number;
  blockCount?: number;
  hasAttachment?: boolean;
  pinned?: boolean;
  color?: string | null;
  rev: number;
  syncRev?: number;
  syncHash?: string | null;
  remoteRev?: number;
  contentHash?: string;
  createdAt: Rfc3339;
  updatedAt: Rfc3339;
  deletedAt?: Rfc3339 | null;
  purgedAt?: Rfc3339 | null;
  createdDevice?: string;
  updatedDevice?: string;
}

/** 列表行：后端不返回正文（列表查询不取文档体）。 */
export interface NoteListRow {
  id: Uuid;
  title: string;
  summary?: string;
  folderId?: Uuid | null;
  folderName?: string;
  pinned?: boolean;
  charCount?: number;
  hasAttachment?: boolean;
  updatedAt: Rfc3339;
  createdAt?: Rfc3339;
  deletedAt?: Rfc3339 | null;
  color?: string | null;
}

/* ------------------------------------------------------------------- 文件夹 */

export interface Folder {
  id: Uuid;
  parentId: Uuid | null;
  name: string;
  color?: string | null;
  sortOrder?: number;
  rev?: number;
  createdAt?: Rfc3339;
  updatedAt?: Rfc3339;
}

export interface FolderNode extends Folder {
  children: FolderNode[];
  noteCount?: number;
}

/* -------------------------------------------------------------------- 检索 */

export interface SearchHit {
  noteId: Uuid;
  score: number;
  titleHit?: boolean;
  /** 已由本地核心转义的片段，可直接渲染。 */
  snippetHtml: SafeHtml;
  matchStarts?: number[];
}

/* -------------------------------------------------------------------- 附件 */

export interface Attachment {
  id: Uuid;
  noteId?: Uuid;
  blockId?: Uuid;
  role?: string;
  sha256?: string;
  mediaType?: string;
  size?: number;
  name?: string;
  /** present | missing | pending —— 缺失时 UI 显示占位而非空白。 */
  state?: string;
  localPath?: string | null;
}

/* -------------------------------------------------------------------- 统计 */

export interface StoreStats {
  notes?: number;
  notesInTrash?: number;
  folders?: number;
  attachments?: number;
  ftsEntries?: number;
  dbBytes?: number;
  searchGeneration?: number;
  inflightOps?: number;
}

/* ------------------------------------------------------------------ 账户配置 */

export type TlsPolicyKind = 'strict' | 'pin' | 'caBundle' | 'insecureLocal';
export type ProxyMode = 'direct' | 'system' | 'http' | 'https' | 'socks5';

export interface ProxyProfile {
  mode?: ProxyMode;
  host?: string;
  port?: number;
  username?: string;
  /** 只写不读：响应里永远不出现明文口令。 */
  password?: string;
  bypass?: string[];
  resolveSystem?: boolean;
}

export interface TlsPolicy {
  kind?: TlsPolicyKind;
  fingerprints?: string[];
  caBundlePem?: string;
}

/**
 * 后端返回的账户视图：凭据以 hasCredential 布尔表达，不回显口令。
 * 字段是**平铺**的（`proxyHost` 而不是 `proxy.host`），翻译集中在 `sync/accountWire.ts`。
 */
export interface Account {
  id?: Uuid;
  label?: string;
  baseUrl?: string;
  rootPrefix?: string;
  /** 用户名不是秘密，回填表单要用它；口令仍然只写不读。 */
  username?: string;
  authKind?: string;
  tlsPolicy?: string;
  proxyMode?: string;
  proxyHost?: string;
  proxyPort?: number;
  proxyUsername?: string;
  bypass?: string[];
  enabled?: boolean;
  hasCredential?: boolean;
  /**
   * §5 探测结果。`null`/缺省 = **还没探过**（区别于 `0` = 探过了，什么都不支持）。
   * 核心的 `Option<u32>` 序列化成 JSON `null`，所以这里必须允许 null。
   */
  capMask?: number | null;
  /** 核心按 §5 的表算出的写入策略；界面不许自己从 capMask 反推。 */
  writeStrategy?: 'S1' | 'S2' | 'S3' | string | null;
  capsProbedAt?: Rfc3339 | null;
  updatedAt?: Rfc3339;
}

/** 提交给后端的账户草稿：口令可选，缺省表示"保留已存的凭据"。 */
export interface AccountDraft {
  id?: Uuid | null;
  /** 核心要求非空；留空时由 `labelFromBaseUrl` 从地址推一个。 */
  label?: string;
  baseUrl: string;
  rootPrefix: string;
  username: string;
  /** 仅在用户输入新口令时携带；空字符串表示不修改。 */
  password?: string;
  tlsPolicy: TlsPolicy;
  proxy: ProxyProfile;
  enabled: boolean;
}

/* ------------------------------------------------------------- 导出 / 导入 */

export interface ExportRequest {
  folderIds?: Uuid[];
  includeAttachments?: boolean;
  includeTrash?: boolean;
  path?: string;
}

export interface ImportRequest {
  path?: string;
  mode?: 'intoEmpty' | 'merge';
}

export interface Report {
  path?: string;  sha256?: string;
  created?: number;
  merged?: number;
  skipped?: number;
  conflicts?: number;
  restoredAttachments?: number;
  counts?: Record<string, number>;
  abortedReason?: string | null;
  ok?: boolean;
}

/** 一致性快照的自证信息：恢复闸门靠 sha256 + userVersion，不靠文件名。 */
export interface BackupInfo {
  path: string;
  sha256: string;
  userVersion: number;
  bytes: number;
  createdAt: string;
}

export interface RestoreOutcome {
  restartRequired: boolean;
  sha256: string;
  userVersion: number;
  path: string;
}

/* ------------------------------------------------------------------ 冲突 */

export interface ConflictCard {
  conflictId: number;
  noteId?: Uuid;
  copyNoteId?: Uuid | null;
  noteTitle?: string;
  title?: string;
  localRev?: number;
  remoteRev?: number;
  localPreview?: string;
  remotePreview?: string;
  createdAt?: Rfc3339;
  blockIds?: Uuid[];
}

/* ---------------------------------------------------------------- 事件总线 */

export type SyncBadgeKind = 'synced' | 'syncing' | 'offline' | 'failed';

export interface SyncProgress {
  done: number;
  total: number;
  bytes?: number;
}

export interface SyncEventPayload {
  kind: 'sync';
  badge: SyncBadgeKind;
  progress?: SyncProgress;
  errorCode?: string;
  messageKey?: string;
}

export interface NotesChangedEventPayload {
  kind: 'notes-changed';
  ids: Uuid[];
}

export interface ConflictEventPayload {
  kind: 'conflict';
  conflictId: number;
  noteTitle?: string;
}

export interface ToastEventPayload {
  kind: 'toast';
  messageKey: string;
  level?: 'info' | 'warn' | 'error';
}

export type UiEvent =
  | SyncEventPayload
  | NotesChangedEventPayload
  | ConflictEventPayload
  | ToastEventPayload;

/* ------------------------------------------------------------- 命令与错误 */

/** 统一命令名（全部经 callCommand 走壳层）。 */
export const Commands = {
  createNote: 'create_note',
  editNote: 'edit_note',
  setNoteFolder: 'set_note_folder',
  setNotePinned: 'set_note_pinned',
  deleteNote: 'delete_note',
  restoreNote: 'restore_note',
  purgeNote: 'purge_note',
  createFolder: 'create_folder',
  renameFolder: 'rename_folder',
  moveFolder: 'move_folder',
  deleteFolder: 'delete_folder',
  listNotes: 'list_notes',
  getNote: 'get_note',
  search: 'search',
  listFolders: 'list_folders',
  attachFile: 'attach_file',
  stats: 'stats',
  syncNow: 'sync_now',
  configureAccount: 'configure_account',
  account: 'account',
  exportData: 'export_data',
  importData: 'import_data',
  backupDb: 'backup_db',
  listBackups: 'list_backups',
  restoreDb: 'restore_db',
  openConflicts: 'open_conflicts',
  resolveConflict: 'resolve_conflict',
  previewText: 'preview_text',
} as const;

export type CommandName = (typeof Commands)[keyof typeof Commands];

/** 后端错误的用户可读描述：UI 只认 messageKey，不展示原始码。 */
export interface UserFacingError {
  id?: string;
  messageKey?: string;
  retryable?: boolean;
  action?: string | null;
}

export interface CommandResult<T = unknown> {
  ok: boolean;
  payload?: T;
  error?: UserFacingError | null;
  traceId?: string;
}
