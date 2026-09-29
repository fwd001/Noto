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
    /// 用户**已经对这一份分歧表过态**时，记下当时对面那一版的编号（P19）。
    /// 编号一模一样才让路；对面又往前走了就照旧算冲突 —— 卡片是按当前分叉每轮重算的，
    /// 少了这条判据就是"按了『保留两份』，它闪一下又回到收件箱"（G17 实测的第二半）。
    pub decided_remote: Option<u64>,
}

impl LocalView {
    pub fn dirty(&self) -> bool {
        self.rev != self.sync_rev
    }
    /// 本机这一行是**已确认的删除**：干净（曾经与对面一致）且带着删除时间。
    /// P20 的判据、"要不要造副本"、"要不要替用户把对面那一版取回来"三处都用这一个定义 ——
    /// 同一条语义分三份写是本仓踩过多次的那类漂移。
    pub fn confirmed_delete(&self) -> bool {
        self.deleted_at.is_some() && !self.dirty()
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

    // P19 已经裁决过的同一份分歧：不再问第二遍，按编号高低走普通的一条路。
    // 裁决时那一条可发布的版本已经被抬到对面之上（ADR-0021 D2 的抬号），所以这里几乎都是 Push；
    // 编号相同（不可能同时又是已裁决的脏行）才 NoOp。对面若又往前走了，`decided_remote`
    // 就对不上 `r.rev`，下面的 P11/P11b/P8 照旧生效 —— 让路只针对"同一件事"，不是永久静音。
    if local_changed && l.decided_remote == Some(r.rev) {
        let action = if l.rev > r.rev {
            Action::Push
        } else if l.rev < r.rev {
            Action::Pull
        } else {
            Action::NoOp
        };
        return Decision {
            key: l.key(),
            action,
            rule: "P19",
        };
    }

    // P20 对面把**本机已经确认删掉**的那条写活了（本机干净、远端是更新的活版本）。
    // 这台设备上必须问一句，不能默默让它回到正常列表 —— §5.1 与 P11 那句"绝不静默二选一"
    // 管的是两侧，不只是发起编辑的那一侧。落库那一侧已经先挡住了（`apply_note` 的墓碑守卫），
    // 所以这里不是在"防覆盖"，而是在**补上那一问**：没有这条规则，用户永远不知道对面动过它。
    if l.confirmed_delete() && r.deleted_at.is_none() && remote_changed {
        return Decision {
            key: l.key(),
            action: Action::Conflict(ConflictKind::DeleteUpdate),
            rule: "P20",
        };
    }

    // P11 删除 vs 修改（双向都算冲突，且本地内容必须先保留）
    // **远端那一版是永久删除的墓碑时也算**：这里以前写着 `!r.purged`，于是"对面永久删除 + 本机把这条
    // 从回收站里恢复了"那一格掉到 P7 的"内容相同即收敛"上 —— 墓碑公告带的 `hash` 就是最后一版的正文哈希，
    // 两边一比就"相同"，本机那一版被静默 PUT 回服务器，**永久删除被同步复活了**（缺口 G24，两设备实测：
    // 卡片 0 张、服务器上那条变成 `purged=false / payload 非空 / 含正文=true`）。§8.4 承诺的是走冲突路径。
    if local_changed && (r.deleted_at.is_some() || r.purged) {
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
        // **`NoOp` 不是收敛。** §1.2 这句承诺的是"两侧最终内容相同就收敛"，而给 `NoOp` 的结果是
        // 谁也不动这一行：两侧都还脏着。两台真设备实测（`notera-host/tests/same_content_convergence.rs`，
        // 先跑是红的）—— A 连改两次（rev 3）、B 一次改成同一份（rev 2），六轮追平之后 B 仍然
        // `dirty_notes=1 / outbox_pending=1`，也就是**设置页那句"待处理任务"永远不掉**，而且每轮白跑一次。
        // 所以按 rev 高低退回两条已经验证过的路：高的一边把记录公告出去（Push），
        // 低的一边把对面那一版拉下来（Pull —— 正文哈希相同，拉下来改变的只是 rev 与标量字段）。
        // 两侧 rev 真相等时才是无事可做（那一条由引擎的 settle 收尾，见 sync/lib.rs 那段"崩在公告与结清之间"）。
        //
        // 为什么**不**用"直接标成已同步"来收这一行：`note.content_hash` 只覆盖正文，
        // **不含 `pinned` / `color` / `folder_id`**（它们在打包时才加进 payload）。
        // 正文相同而标量不同的那一格，"不上传就算完成"会**静默丢掉那条标量变更**。
        let action = if l.rev > r.rev {
            Action::Push
        } else if l.rev < r.rev {
            Action::Pull
        } else {
            Action::NoOp
        };
        return Decision {
            key: l.key(),
            action,
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
            decided_remote: None,
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

    /// P19：用户对**同一个远端 rev** 表过态之后，同一件事不许每轮再问一遍（G17 的第二半 ——
    /// 卡片是按当前分叉重算的，裁决过又回来）。三个方向都要有数：没记决策 → 仍 P11（这条判据
    /// 不许把 P11 弱化）；记在别的编号上（对面又往前走了）→ 仍 P11；记在这一个编号上 → Push。
    #[test]
    fn p19_a_decided_divergence_is_pushed_not_asked_again() {
        let deleted = RemoteView {
            kind: "n".into(),
            id: "x".into(),
            rev: 2,
            hash: Some("sha256:remote".into()),
            deleted_at: Some("2026-09-29T00:00:00Z".into()),
            purged: false,
        };
        let mut local = l(3, 1, "1111");
        assert_eq!(
            decide(Some(&local), Some(&deleted)).rule,
            "P11",
            "没表过态时删除 vs 修改必须仍然是冲突"
        );
        local.decided_remote = Some(9);
        assert_eq!(
            decide(Some(&local), Some(&deleted)).rule,
            "P11",
            "对面又往前走了就必须重开冲突 —— 让路只针对同一件事"
        );
        local.decided_remote = Some(2);
        let d = decide(Some(&local), Some(&deleted));
        assert_eq!(d.rule, "P19");
        assert_eq!(d.action, Action::Push, "裁决过的那一版要能公告出去");
    }

    /// P20：本机**已确认删掉**的一条，被对面写活了（`revival` 由 host 从库里标出来）。
    /// 三个方向都要有数：没标 revival → 走 P6 的普通 Pull（这是过去行为，不许一律变成卡片）；
    /// 标了 revival 且对面确实更新了 → 冲突，让本机这一台来决定；对面也还是删除态 → 不弹卡。
    #[test]
    fn p20_a_confirmed_delete_the_peer_revived_asks_this_device_instead_of_resurrecting() {
        let alive = r(3, "cccc");
        let mut tomb = l(2, 2, "aaaa"); // rev == sync_rev：干净，但"删除"已经公告过
        tomb.deleted_at = Some("2026-09-29T00:00:00Z".into());
        let d = decide(Some(&tomb), Some(&alive));
        assert_eq!(d.rule, "P20");
        assert_eq!(
            d.action,
            Action::Conflict(ConflictKind::DeleteUpdate),
            "对面把我删掉的写活了 → 必须问这台，不许静默复活"
        );
        // 三个方向都不许误弹卡：对面也还是删除态、本机根本没删、本机是"删完又改"（脏墓碑）。
        let still_deleted = RemoteView {
            kind: "n".into(),
            id: "x".into(),
            rev: 3,
            hash: Some("sha256:cccc".into()),
            deleted_at: Some("2026-09-29T00:00:00Z".into()),
            purged: false,
        };
        let both_deleted = decide(Some(&tomb), Some(&still_deleted));
        assert_ne!(
            both_deleted.rule, "P20",
            "对面也还是删除态 → 不是分歧，不许弹卡"
        );
        assert_eq!(
            both_deleted.action,
            Action::ApplyRemoteDelete,
            "两侧都删了就照远端把本地这一行收进回收站（P10）"
        );
        let clean_alive = l(2, 2, "aaaa");
        assert_eq!(
            decide(Some(&clean_alive), Some(&alive)).rule,
            "P6",
            "本机没删、只是对面往前走了一步 → 普通 Pull，不许变成卡片"
        );
        let mut dirty_tomb = tomb.clone();
        dirty_tomb.rev = 3; // 删完之后本机又改过：该由 P11 那一族处理，不是 P20
        assert_ne!(
            decide(Some(&dirty_tomb), Some(&alive)).rule,
            "P20",
            "脏墓碑的本地内容是用户还没公告的编辑，判据不许和 P20 混在一起"
        );
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
        // 两台设备各自打开又保存：rev 都变了，内容一样 → **不许发冲突卡片**。
        // 这一条原来还顺带断言了 `NoOp`，而那个断言是错的（它让这一行永远留在待同步，
        // 两台真设备实测 `dirty=1 / outbox=1`）—— 现在断言"高的一边推、低的一边拉"，
        // 而"不许是冲突"这半边承诺一点没放松。
        let d = decide(Some(&l(7, 5, "same")), Some(&r(8, "same")));
        assert_eq!(d.action, Action::Pull, "内容相同但不许是冲突，也不许是不动");
        assert_eq!(d.rule, "P7");
        assert_ne!(
            d.action,
            Action::Conflict(ConflictKind::UpdateUpdate),
            "同内容永远不该长成一张二选一的卡片"
        );

        // 反方向也要有数：本地那一版 rev 更高（还没公告出去）→ 该推，不是该拉。
        let e = decide(Some(&l(9, 5, "same")), Some(&r(8, "same")));
        assert_eq!(
            e.action,
            Action::Push,
            "本地更高的一版不许被当成'已经同步过'"
        );
        // 两侧 rev 真相等才是无事可做。
        let f = decide(Some(&l(8, 5, "same")), Some(&r(8, "same")));
        assert_eq!(
            f.action,
            Action::NoOp,
            "rev 相同、内容相同 —— 这一格才是真的没有事"
        );
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
            (Action::Pull, "P7"),
            "短哈希与全哈希同值必须算收敛 —— 而收敛是**有动作的**收敛（本地 rev 低 → 拉），\
             不是 `NoOp` 留一行永远待同步"
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
