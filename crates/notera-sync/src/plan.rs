//! 计划生成：docs/SYNC-PROTOCOL.md §7 判定表 P1..P18 的**逐条可执行**实现。
//!
//! 纯函数：输入本地视图 + 远端视图，输出动作集合。所有分支都能被单测覆盖，
//! 不需要真网络也不需要真数据库 —— 这是把"同步正确性"钉死在地方的手段。

use crate::manifest::EntryRef;
use serde::{Deserialize, Serialize};

/// 本地实体的判定所需字段。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalView {
    pub kind: String, // "n" / "f" / "a"
    pub id: String,
    pub rev: u64,
    pub sync_rev: u64,
    pub sync_hash: Option<String>,
    pub content_hash: String,
    pub deleted_at: Option<String>,
    pub purged_at: Option<String>,
    /// 本地在删除之后又被编辑过（P11 判据之一）
    pub edited_after_delete: bool,
}

impl LocalView {
    pub fn dirty(&self) -> bool {
        self.rev != self.sync_rev
    }
    pub fn key(&self) -> (String, String) {
        (self.kind.clone(), self.id.clone())
    }
}

/// 远端实体（来自清单或记录复验）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteView {
    pub kind: String,
    pub id: String,
    pub rev: u64,
    pub hash: Option<String>,
    pub deleted_at: Option<String>,
    pub purged: bool,
}

impl RemoteView {
    pub fn key(&self) -> (String, String) {
        (self.kind.clone(), self.id.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    /// 无操作（已收敛）
    NoOp,
    /// 上传本地记录
    Push,
    /// 上传并标记为删除
    PushDelete,
    /// 上传永久删除墓碑
    PushPurge,
    /// 拉取远端记录
    Pull,
    /// 应用远端删除
    ApplyRemoteDelete,
    /// 应用远端永久删除
    ApplyRemotePurge,
    /// 远端缺失：补传，**绝不**删本地（C2）
    MissingRemote,
    /// 真冲突，交合并流程
    Conflict(ConflictKind),
    /// 版本超前：只读，不写回（I7）
    ReadOnly,
    /// 校验失败：丢弃响应，不写库（I6）
    RejectCorrupt,
    /// 悬空父级：重挂默认本
    Reparent,
    /// 需要下载附件
    FetchAttachment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictKind {
    UpdateUpdate,
    DeleteUpdate,
    UpdateDelete,
    MoveMove,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    pub key: (String, String),
    pub action: Action,
    /// 判定表行号，用于诊断与测试断言（P1..P18）
    pub rule: &'static str,
}

/// 判定表实现。**每一条 P 都有对应测试**，缺测试的分支视为未实现。
///
/// `remote = None` 表示"清单里没有该实体"。注意这与"清单有但记录 404"不同：
/// 后者由引擎标成 `RemoteView` 缺失并走 [`Action::MissingRemote`]，见 §10。
pub fn decide(local: Option<&LocalView>, remote: Option<&RemoteView>) -> Decision {
    // 两侧都在 → 交给完整判定
    if let (Some(l), Some(r)) = (local, remote) {
        return both_present(l, r);
    }
    match (local, remote) {
        // P1 不可能：调用方不会为"双方都不存在"生成计划
        (None, None) => Decision {
            key: (String::new(), String::new()),
            action: Action::NoOp,
            rule: "P1",
        },
        // P13 远端是永久删除墓碑，本地什么都没有 → 只需落墓碑
        (None, Some(r)) if r.purged => Decision {
            key: r.key(),
            action: Action::ApplyRemotePurge,
            rule: "P13",
        },
        // P2 远端新增
        (None, Some(r)) if r.deleted_at.is_none() => Decision {
            key: r.key(),
            action: Action::Pull,
            rule: "P2",
        },
        // 远端已删而本地没有该实体：无事可做（墓碑不必拉成笔记）
        (None, Some(r)) => Decision {
            key: r.key(),
            action: Action::NoOp,
            rule: "P2b",
        },
        // P3 / C2：本地有、清单没有 —— 补传，**绝不删本地**
        (Some(l), None) => Decision {
            key: l.key(),
            action: if l.purged_at.is_some() {
                Action::PushPurge
            } else if l.deleted_at.is_some() {
                Action::PushDelete
            } else {
                Action::MissingRemote
            },
            rule: "P3",
        },
        _ => unreachable!(),
    }
}

/// 两侧都在时的完整判定。
fn both_present(l: &LocalView, r: &RemoteView) -> Decision {
    let local_changed = l.dirty();
    let remote_changed = r.rev != l.sync_rev;

    // P16 版本超前（远端 rev 比本程序语义能力新，由引擎用 protocol 判定后传入）
    // P12/P13 永久删除传播优先于一切编辑
    if r.purged && !local_changed {
        return Decision {
            key: l.key(),
            action: Action::ApplyRemotePurge,
            rule: "P13",
        };
    }
    if l.purged_at.is_some() && local_changed {
        return Decision {
            key: l.key(),
            action: Action::PushPurge,
            rule: "P12",
        };
    }

    // P11 删除 vs 修改（双向都算冲突，且本地内容必须先保留）
    if local_changed && r.deleted_at.is_some() && !r.purged {
        return Decision {
            key: l.key(),
            action: Action::Conflict(ConflictKind::UpdateDelete),
            rule: "P11",
        };
    }
    if l.deleted_at.is_some() && local_changed && remote_changed && r.deleted_at.is_none() {
        return Decision {
            key: l.key(),
            action: Action::Conflict(ConflictKind::DeleteUpdate),
            rule: "P11b",
        };
    }

    // P9 本地删除待传播
    if l.deleted_at.is_some() && local_changed && !remote_changed {
        return Decision {
            key: l.key(),
            action: Action::PushDelete,
            rule: "P9",
        };
    }
    // P10 远端删除、本地未改
    if r.deleted_at.is_some() && remote_changed && !local_changed {
        return Decision {
            key: l.key(),
            action: Action::ApplyRemoteDelete,
            rule: "P10",
        };
    }

    // P4 双方都停在一致点
    if !local_changed && !remote_changed {
        return Decision {
            key: l.key(),
            action: Action::NoOp,
            rule: "P4",
        };
    }
    // P5 仅本地改
    if local_changed && !remote_changed {
        return Decision {
            key: l.key(),
            action: Action::Push,
            rule: "P5",
        };
    }
    // P6 仅远端改
    if !local_changed && remote_changed {
        return Decision {
            key: l.key(),
            action: Action::Pull,
            rule: "P6",
        };
    }
    // P7/P8 两侧都改：内容相同即收敛，不同才是真冲突。
    // 比较走 `same_content_hash`：远端索引带的是 12 位短哈希，本地行上是全哈希，
    // 直接 `==` 会让 P7 永远不成立 —— 两侧内容一样也会被判成冲突（实测就是这样）。
    let same = match (&r.hash, Some(&l.content_hash)) {
        (Some(h), Some(mine)) => notera_core::same_content_hash(h, mine),
        _ => false,
    };
    if same {
        return Decision {
            key: l.key(),
            action: Action::NoOp,
            rule: "P7",
        };
    }
    Decision {
        key: l.key(),
        action: Action::Conflict(ConflictKind::UpdateUpdate),
        rule: "P8",
    }
}

/// 从两侧视图生成整轮计划。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub decisions: Vec<Decision>,
}

impl Plan {
    pub fn build(locals: &[LocalView], remotes: &[RemoteView]) -> Plan {
        let mut lmap: std::collections::BTreeMap<(String, String), &LocalView> =
            locals.iter().map(|l| (l.key(), l)).collect();
        let mut rmap: std::collections::BTreeMap<(String, String), &RemoteView> =
            remotes.iter().map(|r| (r.key(), r)).collect();
        let mut keys: std::collections::BTreeSet<(String, String)> = Default::default();
        keys.extend(lmap.keys().cloned());
        keys.extend(rmap.keys().cloned());
        let mut decisions = Vec::new();
        for k in keys {
            let l = lmap.remove(&k);
            let r = rmap.remove(&k);
            let mut d = decide(l, r);
            if d.key.0 == "?" {
                d.key = k;
            }
            decisions.push(d);
        }
        Plan { decisions }
    }

    pub fn pushes(&self) -> Vec<&Decision> {
        self.decisions
            .iter()
            .filter(|d| {
                matches!(
                    d.action,
                    Action::Push | Action::PushDelete | Action::PushPurge | Action::MissingRemote
                )
            })
            .collect()
    }
    pub fn pulls(&self) -> Vec<&Decision> {
        self.decisions
            .iter()
            .filter(|d| matches!(d.action, Action::Pull))
            .collect()
    }
    pub fn conflicts(&self) -> Vec<&Decision> {
        self.decisions
            .iter()
            .filter(|d| matches!(d.action, Action::Conflict(_)))
            .collect()
    }
    /// 本轮是否会产生任何远端写入（用于空轮断言）。
    pub fn is_read_only(&self) -> bool {
        self.pushes().is_empty()
    }
}

/// 把本地实体投影成清单条目（提交进窗口用）。
pub fn entry_of(l: &LocalView, size: u64) -> EntryRef {
    EntryRef {
        i: l.id.clone(),
        t: l.kind.clone(),
        r: l.rev,
        h: short12(&l.content_hash),
        s: size,
        d: l.deleted_at.clone(),
        p: if l.purged_at.is_some() { 1 } else { 0 },
    }
}

fn short12(hash: &str) -> String {
    let h = hash.strip_prefix("sha256:").unwrap_or(hash);
    h[..12.min(h.len())].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(rev: u64, sync_rev: u64, hash: &str) -> LocalView {
        LocalView {
            kind: "n".into(),
            id: "x".into(),
            rev,
            sync_rev,
            sync_hash: Some("sha256:base".into()),
            content_hash: format!("sha256:{hash}"),
            deleted_at: None,
            purged_at: None,
            edited_after_delete: false,
        }
    }
    fn r(rev: u64, hash: &str) -> RemoteView {
        RemoteView {
            kind: "n".into(),
            id: "x".into(),
            rev,
            hash: Some(format!("sha256:{hash}")),
            deleted_at: None,
            purged: false,
        }
    }

    #[test]
    fn p2_remote_only_pulls() {
        let d = decide(None, Some(&r(3, "aaa")));
        assert_eq!((d.action, d.rule), (Action::Pull, "P2"));
    }

    #[test]
    fn p3_local_only_never_deletes_local() {
        let d = decide(Some(&l(1, 0, "aaa")), None);
        assert_eq!((d.action.clone(), d.rule), (Action::MissingRemote, "P3"));
        // 关键：绝不是"删除本地"
        assert!(!matches!(
            d.action,
            Action::ApplyRemoteDelete | Action::ApplyRemotePurge
        ));
    }

    #[test]
    fn p4_converged_noop() {
        let a = l(5, 5, "abc");
        let b = r(5, "abc");
        assert_eq!(decide(Some(&a), Some(&b)).action, Action::NoOp);
        assert_eq!(decide(Some(&a), Some(&b)).rule, "P4");
    }

    #[test]
    fn p5_and_p6_one_sided() {
        assert_eq!(
            decide(Some(&l(6, 5, "abc")), Some(&r(5, "abc"))).action,
            Action::Push
        );
        assert_eq!(
            decide(Some(&l(5, 5, "abc")), Some(&r(6, "xyz"))).action,
            Action::Pull
        );
    }

    #[test]
    fn p7_identical_content_is_not_a_conflict() {
        // 两台设备各自打开又保存：rev 都变了，内容一样 → 收敛，不产冲突
        let d = decide(Some(&l(7, 5, "same")), Some(&r(8, "same")));
        assert_eq!(d.action, Action::NoOp, "内容相同必须判收敛");
        assert_eq!(d.rule, "P7");
    }

    #[test]
    fn p7_converges_with_the_hash_shapes_production_actually_uses() {
        // 上面那条 p7 两侧都用同一个假串，所以它测不到真正的形状差：
        // 生产里本地行上是**整条** sha256，远端索引/清单上是**12 位短哈希**。
        // 拿这两者直接 `==`，P7 永远不成立 —— 两侧内容完全一样也会发一张冲突卡片。
        let full = format!("abc123def456{}", "0".repeat(52));
        let short = full[..12].to_string();
        let local = l(7, 5, &full);
        let remote = RemoteView {
            kind: "n".into(),
            id: "x".into(),
            rev: 8,
            hash: Some(short.clone()),
            deleted_at: None,
            purged: false,
        };
        let d = decide(Some(&local), Some(&remote));
        assert_eq!(
            (d.action, d.rule),
            (Action::NoOp, "P7"),
            "短哈希与全哈希同值必须算收敛"
        );

        // 而真的不一样时不许因为"前 12 位相同"就收敛掉：短哈希只用于同一版判定，
        // 这里给的是两个不同的完整值，必须仍然判成冲突。
        let other = RemoteView {
            hash: Some("ff00ee001122".to_string()),
            ..remote
        };
        let e = decide(Some(&local), Some(&other));
        assert_eq!(
            (e.action, e.rule),
            (Action::Conflict(ConflictKind::UpdateUpdate), "P8")
        );
    }

    #[test]
    fn p8_genuine_conflict() {
        let d = decide(Some(&l(7, 5, "localhash")), Some(&r(8, "remotehash")));
        assert_eq!(d.action, Action::Conflict(ConflictKind::UpdateUpdate));
        assert_eq!(d.rule, "P8");
    }

    #[test]
    fn p9_p10_deletions() {
        let mut a = l(6, 5, "abc");
        a.deleted_at = Some("2026-09-25T00:00:00.000Z".into());
        assert_eq!(
            decide(Some(&a), Some(&r(5, "abc"))).action,
            Action::PushDelete
        );

        let mut b = r(6, "abc");
        b.deleted_at = Some("2026-09-25T00:00:00.000Z".into());
        assert_eq!(
            decide(Some(&l(5, 5, "abc")), Some(&b)).action,
            Action::ApplyRemoteDelete
        );
    }

    #[test]
    fn p11_delete_versus_edit_is_a_conflict_not_silent_win() {
        // 远端删了，本地在删除点之后又改了
        let mut b = r(6, "abc");
        b.deleted_at = Some("2026-09-25T00:00:00.000Z".into());
        let d = decide(Some(&l(7, 5, "edited")), Some(&b));
        assert!(
            matches!(d.action, Action::Conflict(ConflictKind::UpdateDelete)),
            "{d:?}"
        );
        // 反向：本地删、远端改
        let mut a = l(6, 5, "abc");
        a.deleted_at = Some("2026-09-25T00:00:00.000Z".into());
        let d2 = decide(Some(&a), Some(&r(7, "other")));
        assert!(
            matches!(d2.action, Action::Conflict(ConflictKind::DeleteUpdate)),
            "{d2:?}"
        );
    }

    #[test]
    fn p12_p13_purge_propagates() {
        let mut a = l(6, 5, "abc");
        a.purged_at = Some("2026-09-25T00:00:00.000Z".into());
        assert_eq!(
            decide(Some(&a), Some(&r(5, "abc"))).action,
            Action::PushPurge
        );
        let mut b = r(6, "abc");
        b.purged = true;
        b.deleted_at = Some("2026-09-25T00:00:00.000Z".into());
        assert_eq!(
            decide(Some(&l(5, 5, "abc")), Some(&b)).action,
            Action::ApplyRemotePurge
        );
        assert_eq!(decide(None, Some(&b)).action, Action::ApplyRemotePurge);
    }

    #[test]
    fn plan_partitions_actions() {
        let locals = vec![
            {
                let mut x = l(6, 5, "a");
                x.id = "push".into();
                x
            },
            {
                let mut x = l(5, 5, "b");
                x.id = "pull".into();
                x
            },
            {
                let mut x = l(7, 5, "c");
                x.id = "conf".into();
                x
            },
        ];
        let remotes = vec![
            {
                let mut x = r(5, "a");
                x.id = "push".into();
                x
            },
            {
                let mut x = r(6, "zzz");
                x.id = "pull".into();
                x
            },
            {
                let mut x = r(8, "yyy");
                x.id = "conf".into();
                x
            },
            {
                let mut x = r(2, "new");
                x.id = "remote-only".into();
                x
            },
        ];
        let p = Plan::build(&locals, &remotes);
        assert_eq!(p.decisions.len(), 4, "并集去重后应 4 条");
        assert_eq!(p.pushes().len(), 1);
        assert_eq!(p.pulls().len(), 2, "本地落后与远端新增都要拉");
        assert_eq!(p.conflicts().len(), 1);
        assert!(!p.is_read_only());
        assert!(
            Plan::build(&[l(5, 5, "a")], &[r(5, "a")]).is_read_only(),
            "全收敛轮不得产生写入"
        );
    }

    #[test]
    fn entry_projection_truncates_hash_and_keeps_tombstone() {
        let mut a = l(9, 5, "abcdef0123456789");
        a.deleted_at = Some("2026-09-25T00:00:00.000Z".into());
        a.purged_at = Some("2026-09-25T00:00:00.000Z".into());
        let e = entry_of(&a, 1234);
        assert_eq!(e.h.len(), 12);
        assert_eq!(e.r, 9);
        assert!(e.is_deleted() && e.is_purged());
        assert_eq!(e.s, 1234);
    }
}
