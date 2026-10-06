import { describe, expect, it } from 'vitest';
import { hasMessage, t } from '../i18n';
import { attachmentNotice, localUsable } from './attachmentNotice';

/**
 * 「这颗附件此刻在这台设备上是什么情况」这句话的映射表。
 * 判据全部打在**说了什么**上：4×4 那对词汇逐个走一遍，任何一格不许落空。
 * （同 [[sweep-doc-commitments-against-code]]：决定类文案的判据要打在效果上，不能只验"按钮在不在"。）
 */
const LOCALS = ['missing', 'partial', 'available', 'error', 'absent'];
const REMOTES = ['unknown', 'absent', 'present', 'error'];

describe('attachmentNotice 的取值表', () => {
  it('可用的那几格什么都不说', () => {
    for (const remote of REMOTES) {
      expect(attachmentNotice({ localState: 'available', remoteState: remote }), `available/${remote} 不该有话`).toBeNull();
    }
  });

  it('还没问到账（undefined / null）时不许说"缺"—— 那是把没发生的事报给用户', () => {
    expect(attachmentNotice(undefined)).toBeNull();
    expect(attachmentNotice(null)).toBeNull();
    expect(localUsable(undefined)).toBe(false);
  });

  it('每一格都有交代：可用之外一律给出一句已登记的文案', () => {
    for (const local of LOCALS) {
      if (local === 'available') continue;
      for (const remote of REMOTES) {
        const key = attachmentNotice({ localState: local, remoteState: remote });
        expect(key, `${local}/${remote} 这一格没落空`).not.toBeNull();
        expect(hasMessage(key as string), `${key} 没进词表`).toBe(true);
      }
    }
  });

  it('只有"本机没有 + 服务器有"才允许说『正在等待下载』', () => {
    expect(attachmentNotice({ localState: 'missing', remoteState: 'present' })).toBe('editor.attachmentMissing');
    // 远端还没问过 / 被判定没有 / 坏了 —— 说"正在等待下载"就是许愿。
    for (const remote of ['unknown', 'absent', 'error']) {
      expect(attachmentNotice({ localState: 'missing', remoteState: remote })).toBe('editor.attachmentNotOnDevice');
    }
    // 本机"半"与"坏"也不许被说成在等下载：半份要不要重传、坏了要复算，判据在核心那两条命令里。
    for (const local of ['partial', 'error', 'absent']) {
      expect(attachmentNotice({ localState: local, remoteState: 'present' }), `${local}/present`).toBe('editor.attachmentNotOnDevice');
    }
  });

  it('句子不许替用户下结论：中性那句里不许出现"下载""删除""已修复"这类承诺', () => {
    const text = t('editor.attachmentNotOnDevice');
    expect(text, '这句没进词表（t() 会原样回键名）').not.toBe('editor.attachmentNotOnDevice');
    expect(text).not.toMatch(/下载|删除|修复|完成/);
  });
});
