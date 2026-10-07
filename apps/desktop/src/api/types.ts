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
  /**
   * 角色标记（核心 `FolderDto.systemKind`）。默认本是 `'default'`，普通文件夹是 null。
   * 核心对**非 null 的那一类**拒绝改名/移动/删除，所以界面必须按它决定要不要给按钮 ——
   * 此前 DTO 一直发着这一格，前端类型却没声明、`ensureNode` 也没往下带，
   * 于是"默认本"上那三颗按钮点了只回一句 Constraint 错。
   */
  systemKind?: string | null;
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
  /** 核心回的是**命中标题**（`SearchHitDto.title`）。以前这里写的是 `titleHit?: boolean`
   *  与 `matchStarts?: number[]` —— 核心从来没发过这两个键，TS 与 mock 一起绿灯，
   *  真产物里缺的那一格是 `title`（跨语言契约踩过两次的同一形状）。 */
  title: string;
  /** 已由本地核心转义的片段，可直接渲染。 */
  snippetHtml: SafeHtml;
}

/* -------------------------------------------------------------------- 附件 */

/**
 * 一条附件在**这台设备账上**的那对状态（`attachment_retry` / `attachment_reupload` 的返回）。
 *
 * 界面只读它、不解释它：撤哪一半状态、要不要覆盖服务器，判断全在核心那两条命令里 ——
 * 在这儿再写一份"absent 就该怎么、error 就该怎么"就是 §39 禁的第二套状态机，
 * 而两套判据迟早漂成两种行为（本仓在 kind 词汇上漂过两次，见 TEST-PLAN 记法表那节）。
 */
export interface AttachmentLedgerState {
  sha256: string;
  /** missing | partial | available | error（DATA-MODEL §8）。 */
  localState: string;
  /** unknown | absent | present | error。 */
  remoteState: string;
}

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
  /**
   * 附件写入会推进笔记的 rev（核心改了派生列与引用表），所以命令把新 rev 一起回给
   * 编辑器 —— 不接住它，编辑器随后那次自动保存就带着旧 rev 出发，被判成"在别处被改动"。
   */
  rev?: number;
  localPath?: string | null;
}

/* -------------------------------------------------------------------- 统计 */

/// §4.3 / G87：一轮被"远端少了一大截"那道闸门停下时，核心报回来的那两个数。
export interface DivergenceHeld {
  cachedRecords: number;
  receivedRecords: number;
}

export interface StoreStats {
  notes?: number;
  notesInTrash?: number;
  folders?: number;
  attachments?: number;
  ftsEntries?: number;
  dbBytes?: number;
  searchGeneration?: number;
  inflightOps?: number;
  /** ADR-0012 的只读闸门：这本库比本程序新。§4.2 那句「请升级以编辑」的唯一来源。 */
  libraryReadOnly?: boolean;
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
  /** `pin` 档的指纹表（64 位十六进制，公钥证书 DER 的 sha256）。 */
  fingerprints?: string[];
  /** 只在"这一次保存里要改"时带上；留空 = 保留核心里已存的那份（与口令同一套语义）。 */
  caBundlePem?: string;
  /**
   * 界面专用的一位：核心**不回传 PEM 本体**，只回传"存过没有"（`Account.hasCaPem`）。
   * 没有它，重开设置页时那一格看着像空的，用户会以为自己的根证书被吞了。
   */
  hasStoredCaPem?: boolean;
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
  /** 根证书 PEM 本体不下发（可以有几 KB），只下发"存过没有"给表单当占位提示。 */
  hasCaPem?: boolean;
  /** 指纹不是秘密，原样下发才能编辑。 */
  pinnedSha256?: string[];
  proxyMode?: string;
  proxyHost?: string;
  proxyPort?: number;
  /** 核心回的是"代理用户名存过没有"（本体在凭据项里，回传不了），
   *  与 `hasCredential` / `hasCaPem` 同一套口径。 */
  proxyHasUsername?: boolean;
  bypass?: string[];
  enabled?: boolean;
  hasCredential?: boolean;
  /**
   * 这一轮**真拿得到**口令吗（系统凭据库里有，或本次会话的内存表里有）。
   * 引用挂着而这里是 false = 重启过了 / 换机器了 —— 界面要说的不是"已保存"而是"请重填"。
   */
  credentialLive?: boolean;
  /**
   * 拿到的那一份是不是落在**系统**凭据库里（= 退出后还在）。
   * 缺口 G38 选了 B：没有系统凭据库的平台上口令只活在这次进程里，所以这一位是 false。
   */
  credentialPersistent?: boolean;
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
  /** 导出范围：`full` = 整库，`folders` = 按文件夹的子树包（不能当整库备份用）。 */
  scope?: string;
  scopeFolders?: number;
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

/** 「清除一切」的回执：清了多少张表、回收多少附件字节。 */
export interface EraseOutcome {
  tables: number;
  freedBytes: number;
  restartRequired: boolean;
}

/* ------------------------------------------------------------------ 冲突 */

export interface ConflictCard {
  conflictId: number;
  noteId?: Uuid;
  copyNoteId?: Uuid | null;
  /** 副本笔记的 rev：左栏（你这一版）按 `(copyNoteId, copyRev)` 取预览，见 stores/conflicts.ts */
  copyRev?: number | null;
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

/**
 * 同步徽标的可见态。
 *
 * `idle` = **没有配置同步账户**，不是"同步失败"，也不是"正在同步"。
 * 这一态是单独加出来的：原先只有四态时，"没配账户"会落回 `syncing`，
 * 于是界面上出现一颗永远转不停的圈（用户反馈"不知道是不是历史数据"）。
 */
export type SyncBadgeKind = 'synced' | 'syncing' | 'offline' | 'failed' | 'idle';

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
  dailyNote: 'daily_note',
  editNote: 'edit_note',
  setNoteFolder: 'set_note_folder',
  setNotePinned: 'set_note_pinned',
  deleteNote: 'delete_note',
  restoreNote: 'restore_note',
  purgeNote: 'purge_note',
  createFolder: 'create_folder',
  renameFolder: 'rename_folder',
  deleteFolder: 'delete_folder',
  listNotes: 'list_notes',
  getNote: 'get_note',
  search: 'search',
  listFolders: 'list_folders',
  attachFile: 'attach_file',
  attachmentData: 'attachment_data',
  // 坏图/坏附件占位上的两个用户动作。名字与载荷都是核心那两条命令的逐字契约。
  attachmentRetry: 'attachment_retry',
  attachmentReupload: 'attachment_reupload',
  /** 读侧批量问账：这篇笔记引用的每个对象，本机到底有没有可用字节。只读账、不下载字节 ——
   *  一颗芯片的显示判据不该触发一次 32 MiB 的读盘。 */
  attachmentStates: 'attachment_states',
  stats: 'stats',
  syncNow: 'sync_now',
  /** 同步的那几项事实（阶段、**上一次成功时间**、待处理数、开放冲突数、能否重试）。
   *  后端一直有，前端此前从没调过 ⇒ "上一次：{时间}" 那一格只能等本次会话跑完一轮才有值。 */
  syncStatus: 'sync_status',
  acceptDivergence: 'accept_divergence',
  configureAccount: 'configure_account',
  /** 核心早就有这条命令，但界面**从来没有入口** ⇒ 用户想停掉同步只能"清除一切数据"
   *  （连笔记一起删掉）。删账户与删库是两件事。 */
  removeAccount: 'remove_account',
  account: 'account',
  exportData: 'export_data',
  importData: 'import_data',
  importFiles: 'import_files',
  backupDb: 'backup_db',
  listBackups: 'list_backups',
  restoreDb: 'restore_db',
  // 「清除一切数据恢复初始化」。**不可撤销**，命令面要求显式 `confirmed: true`。
  eraseAllData: 'erase_all_data',
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
