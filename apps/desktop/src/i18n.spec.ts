/**
 * 命令层错误码必须落到**具体**文案。
 *
 * 为什么钉这一条：`CmdError.message_key` 是 `cmd.<code>`，而 `messageFor` 会剥掉已知
 * 前缀后再查 `error.<code>`。前缀表漏一项，那一类错误就全体退化成
 * "操作没有成功，可以稍后再试" —— 用户分不清"本机数据有风险"和"网络抖了一下"，
 * 而这正是这个 App 最不允许的模糊。
 */
import { describe, expect, it } from 'vitest';
import { messageFor } from './i18n';

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
  'multi_account_unsupported',
  'invalid_account',
  'save_failed',
  'bad_device',
  'net_config',
  'proxy_credentials_pending',
  'serialize',
  'handler_panic',
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
