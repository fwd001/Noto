/**
 * §6 那一格「备份选择器」：核心早就有自校验、按时间倒序、坏档跳过的清单接口
 * （`list_backups`），而前端只在 `Commands` 里登了名、一次没调过 —— 于是"从备份恢复"
 * 要用户**手敲绝对路径**。这里盯的是那条调用边与"前端不许自己再加工"这两件事。
 *
 * 界面上渲染成什么样、点下去出不出浮层，由 `verify-layout` 第 ㊳ 腿在真浏览器里量
 * （挂载整个设置页要 stub 六个 store，测出来的形状也不如真的渲染一遍可信）。
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { stubLocalService } from './testing/http';
import { useSettingsStore } from './stores/settings';
import { stampToIso } from './util/format';

/** 核心回的形状（`BackupInfo`，serde 已 camelCase）。时间戳是文件 mtime 造的紧凑 UTC 串。 */
function backup(path: string, stamp: string, bytes: number, userVersion: number) {
  return { path, sha256: 'a'.repeat(64), userVersion, bytes, createdAt: stamp };
}

const NEWER = backup('D:/notera/backups/notera-20261007T091530Z.sqlite', '20261007T091530Z', 1_048_576, 7);
const OLDER = backup('D:/notera/backups/notera-20261001T080000Z.sqlite', '20261001T080000Z', 900_000, 6);

describe('备份清单的调用边', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.useRealTimers();
  });

  it('loadBackups 发的就是 list_backups，并把核心回的两份原样搬进来', async () => {
    const service = stubLocalService({ list_backups: () => ({ ok: true, payload: [NEWER, OLDER] }) });
    const settings = useSettingsStore();
    await settings.loadBackups();
    expect(service.callsOf('list_backups')).toHaveLength(1);
    expect(settings.backups.map((b) => b.path)).toEqual([NEWER.path, OLDER.path]);
    expect(settings.backupsFailed).toBe(false);
  });

  it('顺序是核心的事：这里给"旧在前"，界面拿到的也必须是"旧在前"（前端再排一次就是第二套真相）', async () => {
    stubLocalService({ list_backups: () => ({ ok: true, payload: [OLDER, NEWER] }) });
    const settings = useSettingsStore();
    await settings.loadBackups();
    expect(settings.backups[0]?.path).toBe(OLDER.path);
  });

  it('取清单失败要说得出"没取到"，并且不许留下半截清单', async () => {
    // 没注册 handler ⇒ 桩回 404，等价于"命令发不出去/核心没起"
    stubLocalService({});
    const settings = useSettingsStore();
    settings.backups.push(NEWER);
    await settings.loadBackups();
    expect(settings.backupsFailed).toBe(true);
    expect(settings.backups).toHaveLength(0);
  });

  it('备份成功之后立刻重取清单：刚做出来的那一份必须马上可选', async () => {
    const service = stubLocalService({
      backup_db: () => ({ ok: true, payload: NEWER }),
      list_backups: () => ({ ok: true, payload: [NEWER] }),
    });
    const settings = useSettingsStore();
    await settings.loadBackups();
    const before = service.callsOf('list_backups').length;
    await settings.backupDb();
    expect(service.callsOf('list_backups').length).toBeGreaterThan(before);
    expect(settings.backups[0]?.path).toBe(NEWER.path);
  });

  it('按清单恢复发的是那一份的 path，一个字符都不改', async () => {
    const service = stubLocalService({
      list_backups: () => ({ ok: true, payload: [NEWER, OLDER] }),
      restore_db: () => ({ ok: true, payload: { restartRequired: true, sha256: 'b'.repeat(64), path: OLDER.path } }),
    });
    const settings = useSettingsStore();
    await settings.loadBackups();
    await settings.restoreDb(settings.backups[1]?.path ?? '');
    expect(service.callsOf('restore_db')).toHaveLength(1);
    expect(service.lastArgsOf('restore_db')).toEqual({ path: OLDER.path });
  });
});

describe('备份时间戳的读法', () => {
  it('紧凑 UTC 串转成 Date 认得的形式（转不动就会静默变空串，那一格就没时间了）', () => {
    expect(stampToIso('20261007T091530Z')).toBe('2026-10-07T09:15:30Z');
    expect(Number.isNaN(new Date(stampToIso('20261007T091530Z')).getTime())).toBe(false);
  });

  it('认不出来的形状原样返回 —— 宁可看见生串，不许显示空白', () => {
    expect(stampToIso('unknown')).toBe('unknown');
    expect(stampToIso('')).toBe('');
  });
});
