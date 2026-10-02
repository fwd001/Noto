/**
 * 命令层错误码必须落到**具体**文案。
 *
 * 为什么钉这一条：`CmdError.message_key` 是 `cmd.<code>`，而 `messageFor` 会剥掉已知
 * 前缀后再查 `error.<code>`。前缀表漏一项，那一类错误就全体退化成
 * "操作没有成功，可以稍后再试" —— 用户分不清"本机数据有风险"和"网络抖了一下"，
 * 而这正是这个 App 最不允许的模糊。
 */
import { describe, expect, it } from 'vitest';
import { hasMessage, MESSAGE_KEYS, messageFor } from './i18n';

const GENERIC = messageFor('__definitely_not_a_key__');

/** 与 crates/notera-host/src/commands.rs 的 CmdError::of(...) 逐条对齐。 */
const COMMAND_CODES = [
  'not_found',
  'stale_edit',
  'db_too_new',
  'constraint',
  'storage',
  'bad_args',
  'bad_id',
  'unknown_command',
  'no_account',
  // 配好了账户、这一轮只是拿不到口令 —— 与 `no_account` 是两件事（缺口 G38 选 B 之后
  // 在没有系统凭据库的平台上这是每次重启的正常状态）。
  'sync_needs_credentials',
  'multi_account_unsupported',
  'invalid_account',
  'save_failed',
  'bad_device',
  'net_config',
  'proxy_credential_missing',
  // 这两条凭据码是 0.0.31 补的：它们当时**不在 arch-check 的扫描结果里**，因为 Rust 侧写的是
  // `CmdError::of(e.code())` —— 算出来的码那条门禁看不见，于是"漏登记文案"既不报错也不红，
  // 用户只会看到那句通用兜底。现在码写成字面量（被第 12 条看见），并由第 30 条
  // `hygiene:error-code-must-be-literal` 守住"不许再写算出来的码"这一形。
  // 第三条 `credential_unavailable` 随缺口 G38 选 B 一起删了：没有系统凭据库的平台上
  // 口令退到本次会话的内存表，"存不了"这条错误再也不会发生 —— 留一条永远不会出现的码，
  // 就是留一段永远不会被看到的文案。
  'credential_too_long',
  'credential_store_failed',
  'serialize',
  'handler_panic',
  // 下面这批是新增门禁（arch-check `hygiene:rust-error-codes-registered`）从 Rust 侧
  // 扫出来、而这份手抄表当时漏掉的 —— 漏掉的后果不是报错，是那类错误全体退化成
  // "操作没有成功，可以稍后再试"。两边都要有：arch-check 管登记，这里管真拿得到文案。
  'read_failed',
  'attachment_missing',
  'attachment_corrupt',
  'too_large',
  'no_default_folder',
  'bad_action',
  'sync_refused',
  // 0.0.46（缺口 G33）：凭据错了原先被折进 sync_refused + "暂时读不到协议信息，请稍后重试"，
  // 于是它有自己的码之后必须也有自己的文案 —— 用户下一步该做的是改口令，不是等。
  'sync_auth_failed',
  'sync_busy',
  'unknown_account',
  // 「重试取回 / 重新上传本机这份」这两条用户动作的失败面（核心侧 App::retry_attachment /
  // reupload_attachment）。它们必须各有一条自己的文案：这三格长得几乎一样，但用户下一步
  // 该做的事完全不同（等同步 / 改用另一个按钮 / 从别的设备重插一次）。
  'attachment_not_registered',
  'nothing_to_retry',
  'nothing_to_upload',
];

describe('命令层错误文案', () => {
  it.each(COMMAND_CODES)('cmd.%s 有专门文案，不落到通用兜底', (code) => {
    const text = messageFor(`cmd.${code}`);
    expect(text).not.toBe(GENERIC);
    expect(text.length).toBeGreaterThan(6);
  });

  it.each(['offline', 'server_unavailable', 'transport_unreachable', 'timeout'])(
    '裸 code %s 也能解析（同步事件只带 error_code，没有前缀）',
    (code) => {
      expect(messageFor(code)).not.toBe(GENERIC);
    },
  );

  it('未知 code 才允许兜底，且兜底本身可读', () => {
    expect(GENERIC).not.toContain('__');
    expect(GENERIC.length).toBeGreaterThan(6);
  });
});

/**
 * 源码里用到的每个文案键都必须已登记。
 *
 * 为什么只能靠扫源码：`MessageKey` 是 `string` 的别名，`t()` 又把"查不到"处理成
 * "原样返回键名"。于是拼错键、或者用 `t(\`editor.block${Type}\`)` 拼出一个没登记的键，
 * 编译期和运行期都不报错 —— 实测桌面上就印过 "editor.blockCodeBlock"、设置页印过
 * "settings.rootPrefix"。类型系统帮不上忙，就把这条做成门禁。
 */
const NAMESPACES = new Set(MESSAGE_KEYS.map((key) => key.split('.')[0] ?? ''));

/** 全部前端源码原文。用 import.meta.glob 而不是 node:fs：这个项目没装 @types/node，
 *  而且 glob 在 vite 里天然认得 .vue，不必自己处理路径。 */
const SOURCES = import.meta.glob('./**/*.{ts,vue}', { query: '?raw', import: 'default', eager: true }) as Record<string, string>;
const files = Object.entries(SOURCES).filter(([path]) => !path.endsWith('.spec.ts') && !path.endsWith('i18n.ts'));

/** 形如 `editor.blockCode` 的字符串字面量，且命名空间确实是文案命名空间才算。
 *  只认两种位置：t()/messageFor() 的实参，以及 `某字段: 'x.y'` 这样的表值 ——
 *  因为 `v-model="settings.draft.baseUrl"` 也是"引号里带点"，一起扫会全是假阳性。 */
function keysIn(text: string): string[] {
  const found: string[] = [];
  const patterns = [
    /(?:\bt|\bmessageFor)\(\s*['"`]([^'"`]+)['"`]/g,
    /\b[\w-]+\s*:\s*['"]([^'"]+)['"]/g,
  ];
  for (const pattern of patterns) {
    for (const match of text.matchAll(pattern)) {
      const literal = match[1];
      // 必须带点：`view: 'sidebar'` 的值和命名空间同名，不要求点就会被当成键误报
      if (literal && literal.includes('.') && NAMESPACES.has(literal.split('.')[0] ?? '')) found.push(literal);
    }
  }
  return found;
}

const usedKeys = new Set(files.flatMap(([, text]) => keysIn(text)));

describe('文案键登记完整性', () => {
  it('扫到了足量的键与文件（防扫描器自己失效变成常绿）', () => {
    expect(files.length).toBeGreaterThan(15);
    expect(usedKeys.size).toBeGreaterThan(80);
  });

  it('扫描器认得它声称认得的两种位置（否则上面的门禁是假的）', () => {
    // 用双引号数组拼，不用模板字符串：里面的 ${...} 会被当真插值吃掉
    const sample = [
      "<p>{{ t('editor.a') }}</p>",
      '<input v-model="settings.draft.baseUrl" :disabled="settings.dataBusy" />',
      "<button ${messageFor('error.bad_id')}>x</button>",
      "const items = [{ type: 'sidebar', label: 'tb.bold', hintKey: 'slash.headingHint' }];",
      "const view = { name: 'sync' };",
    ].join('\n');
    expect(keysIn(sample).sort()).toEqual(['editor.a', 'error.bad_id', 'slash.headingHint', 'tb.bold']);
  });

  it('用到的键全部已登记，UI 上不会印出键名', () => {
    const missing = [...usedKeys].filter((key) => !hasMessage(key)).sort();
    expect(missing).toEqual([]);
  });
});
