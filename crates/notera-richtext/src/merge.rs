//! 块级三方合并（CONFLICT-RESOLUTION.md §3）。
//!
//! 铁律（§0）：**用户输入过的内容不会因为同步、冲突、崩溃或另一台设备的操作而静默消失**。
//! 所以本模块的原则是"能证明无损才自动合并，否则一律上报冲突"，绝不是"看起来差不多就合了"。
//!
//! [`merge`] 是纯函数：不读时钟、不碰 IO、不 panic（畸形输入也只会返回某个 [`MergeOutcome`]）。
//! 合并产物必须重新 `normalize` + `validate`，不通过就**丢弃产物**退回 `Conflict`（§3.4）。
//!
//! 所有"两侧对称"的抉择（谁当幸存者、追加段先后）一律由**内容**决定，不由"谁是 local"
//! 决定 —— 否则两台设备各算出一份、下一轮又冲突（INV-15 的收敛前提）。

use crate::codec::{block_canonical, canonical, count_changed_blocks, normalize, validate};
use crate::model::{Block, BlockType, Document, Inline, Mark, RichError, supports};
use std::collections::{BTreeMap, BTreeSet};

/// 只读降级的原因（I7 / FWD-03）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadOnlyReason {
    /// 文档版本比本客户端支持的 `DOC_FORMAT` 新：可以看，**不可以写**。
    DocVersionTooNew(u16),
}

/// 三方合并的结果。
#[derive(Clone, Debug, PartialEq)]
pub enum MergeOutcome {
    /// 两侧最终内容相同（回环同步、两台设备各自打开又保存）。
    Converged,
    /// 无损自动合并成功。`taken_*` = 该侧相对 base 有变化并进入结果的顶层块数，
    /// 供 `sync_conflicts.auto_merged` 审计；它不代表"谁赢了"。
    AutoMerged {
        doc: Document,
        taken_local: u32,
        taken_remote: u32,
    },
    /// 存在无法证明无损的块。**不返回文档**：调用方按 §6 走"远端为正文 + 本地为副本"。
    Conflict {
        /// 冲突块 id（文档顺序、去重）。
        conflicting_block_ids: Vec<String>,
    },
    /// 版本闸门：禁止任何写回。
    ReadOnly { because: ReadOnlyReason },
}

/// 哪一侧的内容进入了合并结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Contrib {
    Local,
    Remote,
    Both,
}

/// 块级三方合并（§3.2）。
pub fn merge(base: &Document, local: &Document, remote: &Document) -> MergeOutcome {
    // ── ① 版本闸门（§2 流水线第 ① 步）。
    let max_v = base.v.max(local.v).max(remote.v);
    if !supports(max_v) {
        return MergeOutcome::ReadOnly {
            because: ReadOnlyReason::DocVersionTooNew(max_v),
        };
    }

    // 算法假设输入已 normalize（§3.2 前置条件），不能相信调用方，自己再来一遍。
    let mut b = base.clone();
    let mut l = local.clone();
    let mut r = remote.clone();
    normalize(&mut b);
    normalize(&mut l);
    normalize(&mut r);

    // ── ② 文档级：两侧最终内容相同 → 收敛。
    let cb = canonical(&b);
    let cl = canonical(&l);
    let cr = canonical(&r);
    if cl == cr {
        return MergeOutcome::Converged;
    }
    if cl == cb {
        // 只有远端改过（= pull）：产物就是远端，无损。
        let taken = count_changed_blocks(&b, &r);
        return finish(with_v(r, max_v), &b, &cb, 0, taken);
    }
    if cr == cb {
        let taken = count_changed_blocks(&b, &l);
        return finish(with_v(l, max_v), &b, &cb, taken, 0);
    }

    // 输入自身有"同 id 不同内容"的块 → 合并锚点失效。normalize 无权合并它们
    // （丢任何一边都是丢内容），这里升级为冲突。
    let mut bad = dup_ids(&b);
    bad.extend(dup_ids(&l));
    bad.extend(dup_ids(&r));
    if !bad.is_empty() {
        return MergeOutcome::Conflict {
            conflicting_block_ids: bad,
        };
    }

    let bi = index(&b);
    let li = index(&l);
    let ri = index(&r);

    let mut conflicts: Vec<String> = Vec::new();
    let mut chosen: BTreeMap<String, Block> = BTreeMap::new();
    let (mut taken_local, mut taken_remote) = (0u32, 0u32);

    // ── 2. 逐个 base 块（§3.2 第 2 步）。
    for bo in &b.content {
        let id = bo.id.clone();
        let hb = block_canonical(bo);
        match (li.get(&id), ri.get(&id)) {
            (Some(lb), Some(rb)) => {
                let hl = block_canonical(lb);
                let hr = block_canonical(rb);
                if hl == hb {
                    if hr != hb {
                        taken_remote += 1;
                    }
                    chosen.insert(id, (*rb).clone());
                } else if hr == hb {
                    taken_local += 1;
                    chosen.insert(id, (*lb).clone());
                } else if hl == hr {
                    // 两侧改得一模一样：两份贡献都记上，计数必须与"谁是 local"无关。
                    taken_local += 1;
                    taken_remote += 1;
                    chosen.insert(id, (*lb).clone());
                } else if let Some((blk, c)) = degrade(bo, lb, rb) {
                    count(&mut taken_local, &mut taken_remote, c);
                    chosen.insert(id, blk);
                } else {
                    conflicts.push(id);
                }
            }
            (Some(lb), None) => {
                // 远端删了。本地未改 → 采纳删除；本地改了 → 删除 vs 修改（§1.2/C4）。
                if block_canonical(lb) != hb {
                    conflicts.push(id);
                }
            }
            (None, Some(rb)) => {
                if block_canonical(rb) != hb {
                    conflicts.push(id);
                }
            }
            (None, None) => {
                // 两侧都删：无争议。
            }
        }
    }

    // ── 3. 新增块（不在 base，§3.2 第 3 步）。
    let mut news: BTreeMap<String, Block> = BTreeMap::new();
    for nb in &l.content {
        if bi.contains_key(&nb.id) {
            continue;
        }
        match ri.get(&nb.id) {
            Some(rnb) => {
                if nb.same_content(rnb) {
                    // 两侧各加了一模一样的块。
                    taken_local += 1;
                    taken_remote += 1;
                    news.insert(nb.id.clone(), nb.clone());
                } else if let Some((blk, c)) = degrade(&Block::blank(), nb, rnb) {
                    count(&mut taken_local, &mut taken_remote, c);
                    news.insert(nb.id.clone(), blk);
                } else {
                    // 两台设备生成了同一个 id 却是不同内容：无法判定，冲突。
                    conflicts.push(nb.id.clone());
                }
            }
            None => {
                taken_local += 1;
                news.insert(nb.id.clone(), nb.clone());
            }
        }
    }
    for nb in &r.content {
        if bi.contains_key(&nb.id) || news.contains_key(&nb.id) {
            continue;
        }
        taken_remote += 1;
        news.insert(nb.id.clone(), nb.clone());
    }
    // 同文本不同 id 的新块 → 去重保留一个（§3.2 第 3 步末）。幸存者 = 最小 id，
    // 只看内容，因此两台设备会选出同一个幸存者。
    for (drop, _keep) in dedupe_new_by_text(&news) {
        news.remove(&drop);
    }

    if !conflicts.is_empty() {
        return MergeOutcome::Conflict {
            conflicting_block_ids: unique_in_doc_order(&conflicts, &[&l, &r]),
        };
    }

    // ── 4. 顺序：base 顺序为骨架（§3.2 第 4 步）。
    let skeleton: Vec<String> = b
        .content
        .iter()
        .map(|x| x.id.clone())
        .filter(|id| chosen.contains_key(id))
        .collect();
    let kept: BTreeSet<&String> = skeleton.iter().collect();
    let local_seq: Vec<String> = l
        .content
        .iter()
        .filter(|x| kept.contains(&x.id))
        .map(|x| x.id.clone())
        .collect();
    let remote_seq: Vec<String> = r
        .content
        .iter()
        .filter(|x| kept.contains(&x.id))
        .map(|x| x.id.clone())
        .collect();
    let mut edges: Vec<(String, String)> = Vec::new();
    if local_seq != skeleton {
        edges.extend(chain(&local_seq));
    }
    if remote_seq != skeleton {
        edges.extend(chain(&remote_seq));
    }
    let ordered = match toposort(skeleton, edges) {
        Some(o) => o,
        None => {
            return MergeOutcome::Conflict {
                conflicting_block_ids: involved_in_order_conflict(&local_seq, &remote_seq),
            };
        }
    };
    let pos: BTreeMap<&String, usize> = ordered.iter().enumerate().map(|(i, id)| (id, i)).collect();

    // 新块锚点：在它那一侧紧跟在第几个存活 base 块之后；两侧都有则取较小锚点（对称）。
    let mut anchors: BTreeMap<String, usize> = BTreeMap::new();
    for doc in [&l, &r] {
        let mut gap = 0usize;
        for blk in &doc.content {
            if let Some(p) = pos.get(&blk.id) {
                gap = *p + 1;
                continue;
            }
            if news.contains_key(&blk.id) {
                let e = anchors.entry(blk.id.clone()).or_insert(gap);
                *e = (*e).min(gap);
            }
        }
    }
    let mut anchors: Vec<(usize, String)> = anchors.into_iter().map(|(id, g)| (g, id)).collect();
    anchors.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

    let mut out: Vec<Block> = Vec::with_capacity(ordered.len() + anchors.len());
    let mut it = anchors.into_iter().peekable();
    for (gap, id) in ordered.iter().enumerate() {
        while let Some((g, _)) = it.peek() {
            if *g <= gap {
                let (_, nid) = it.next().expect("peeked");
                if let Some(nb) = news.get(&nid) {
                    out.push(nb.clone());
                }
            } else {
                break;
            }
        }
        if let Some(blk) = chosen.get(id) {
            out.push(blk.clone());
        }
    }
    for (_, nid) in it {
        if let Some(nb) = news.get(&nid) {
            out.push(nb.clone());
        }
    }

    // ── 5. 失败保护（§3.4）。
    let doc = Document {
        v: max_v,
        content: out,
    };
    finish(doc, &b, &cb, taken_local, taken_remote)
}

/// §3.4 的统一出口：**任何** AutoMerged 产物（包括"只有一侧改过"的短路分支）都必须
/// 重新 normalize + validate；过不了就丢弃产物、退回冲突。
///
/// 这一条不能省：调用方拿到 `AutoMerged` 就会写回权威表（I6），而"只有一侧改过"并不
/// 保证那一侧的文档本身是合法的 —— 本机库里躺着一条历史脏数据、或者远端送来一份改了
/// 结构但没过我们这版校验的记录，都会在这里被拦住而不是扩散出去。
fn finish(doc: Document, base: &Document, cb: &str, taken_local: u32, taken_remote: u32) -> MergeOutcome {
    let mut doc = doc;
    normalize(&mut doc);
    if let Err(e) = validate(&doc) {
        return conflict_for_invalid_merge(&doc, base, &e);
    }
    if canonical(&doc) == cb {
        // 声称两侧都改过、结果却等于 base：等于什么都没剩下，宁可上报冲突。
        return MergeOutcome::Conflict {
            conflicting_block_ids: doc.content.iter().map(|x| x.id.clone()).collect(),
        };
    }
    MergeOutcome::AutoMerged {
        doc,
        taken_local,
        taken_remote,
    }
}

fn count(l: &mut u32, r: &mut u32, c: Contrib) {
    match c {
        Contrib::Local => *l += 1,
        Contrib::Remote => *r += 1,
        Contrib::Both => {
            *l += 1;
            *r += 1;
        }
    }
}

fn with_v(mut d: Document, v: u16) -> Document {
    d.v = v;
    d
}

fn index(doc: &Document) -> BTreeMap<String, &Block> {
    let mut m = BTreeMap::new();
    for b in &doc.content {
        m.entry(b.id.clone()).or_insert(b);
    }
    m
}

/// 内容不同的同 id 块（一个文档内部就自相矛盾）。
fn dup_ids(doc: &Document) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut dup = Vec::new();
    for b in &doc.content {
        if !seen.insert(b.id.clone()) && !dup.contains(&b.id) {
            dup.push(b.id.clone());
        }
    }
    dup
}

fn unique_in_doc_order(ids: &[String], docs: &[&Document]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for doc in docs {
        for b in &doc.content {
            if ids.contains(&b.id) && !out.contains(&b.id) {
                out.push(b.id.clone());
            }
        }
    }
    for id in ids {
        if !out.contains(id) {
            out.push(id.clone());
        }
    }
    out
}

fn chain(seq: &[String]) -> Vec<(String, String)> {
    seq.windows(2).map(|w| (w[0].clone(), w[1].clone())).collect()
}

/// 两条侧序链的并集做拓扑排序；有环（= 两侧都调整了顺序且互相矛盾）返回 `None`。
/// tie-break = (base 位置, id)，因此结果确定（INV-15）。
fn toposort(nodes: Vec<String>, edges: Vec<(String, String)>) -> Option<Vec<String>> {
    if nodes.len() < 2 {
        return Some(nodes);
    }
    let mut succ: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut indeg: BTreeMap<String, usize> = nodes.iter().map(|id| (id.clone(), 0usize)).collect();
    for (a, bx) in edges {
        if !indeg.contains_key(&a) || !indeg.contains_key(&bx) {
            continue;
        }
        if succ.entry(a.clone()).or_default().insert(bx.clone()) {
            *indeg.entry(bx).or_insert(0) += 1;
        }
    }
    let base_pos: BTreeMap<String, usize> = nodes.iter().cloned().enumerate().map(|(i, id)| (id, i)).collect();
    let mut ready: Vec<String> = indeg
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(id, _)| id.clone())
        .collect();
    sort_ready(&mut ready, &base_pos);
    let mut out = Vec::with_capacity(nodes.len());
    while let Some(id) = ready.pop() {
        out.push(id.clone());
        let mut touched = false;
        for n in succ.get(&id).cloned().unwrap_or_default() {
            if let Some(d) = indeg.get_mut(&n) {
                *d -= 1;
                if *d == 0 {
                    ready.push(n.clone());
                    touched = true;
                }
            }
        }
        if touched {
            sort_ready(&mut ready, &base_pos);
        }
    }
    if out.len() == nodes.len() {
        Some(out)
    } else {
        None
    }
}

fn sort_ready(ready: &mut Vec<String>, base_pos: &BTreeMap<String, usize>) {
    ready.sort_by(|a, b| {
        let ra = base_pos.get(a).copied().unwrap_or(usize::MAX);
        let rb = base_pos.get(b).copied().unwrap_or(usize::MAX);
        ra.cmp(&rb).then_with(|| a.cmp(b))
    });
    ready.dedup();
}

/// 顺序矛盾涉及的块 id。
fn involved_in_order_conflict(local_seq: &[String], remote_seq: &[String]) -> Vec<String> {
    let pos = |seq: &[String], id: &str| seq.iter().position(|x| x == id);
    let mut out: Vec<String> = Vec::new();
    for (i, a) in local_seq.iter().enumerate() {
        for bx in local_seq.iter().skip(i + 1) {
            let (Some(pa), Some(pb)) = (pos(remote_seq, a), pos(remote_seq, bx)) else {
                continue;
            };
            if pa > pb {
                for id in [a, bx] {
                    if !out.contains(id) {
                        out.push(id.clone());
                    }
                }
            }
        }
    }
    if out.is_empty() {
        out.extend(local_seq.iter().cloned());
    }
    out
}

/// 同 (类型, 纯文本) 的新块去重：返回 (被丢弃 id, 保留 id)。空文本块不参与（那是
/// 用户的空行，两块都算内容）。
fn dedupe_new_by_text(news: &BTreeMap<String, Block>) -> Vec<(String, String)> {
    let mut groups: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for (id, blk) in news {
        let text = blk.plain_text();
        if text.trim().is_empty() {
            continue;
        }
        groups
            .entry((blk.type_.wire_name(), text))
            .or_default()
            .push(id.clone());
    }
    let mut drops = Vec::new();
    for ids in groups.into_values() {
        if ids.len() < 2 {
            continue;
        }
        let mut ids = ids;
        ids.sort();
        let keep = ids[0].clone();
        for id in &ids[1..] {
            drops.push((id.clone(), keep.clone()));
        }
    }
    drops
}

/// base 块两侧都被改动且改法不同 → 依次尝试 M1..M4（§3.3），全失败返回 `None`（M5）。
fn degrade(base: &Block, local: &Block, remote: &Block) -> Option<(Block, Contrib)> {
    let tb = base.plain_text();
    let tx = local.plain_text();
    let ty = remote.plain_text();

    // ── M1：只是格式变化（纯文本相同，marks/attrs 不同）→ 采纳带格式的一侧。
    if tx == ty {
        let (winner, side) = richer(local, remote);
        let mut out = winner.clone();
        out.attrs = merge_attrs(base, local, remote);
        return Some((out, side));
    }

    // ── M2：一侧是另一侧的严格超集（base ⊂ 子集 ⊂ 超集，逐字符包含）。
    if contains(&ty, &tx) && contains(&tx, &tb) {
        let mut out = remote.clone();
        out.attrs = merge_attrs(base, local, remote);
        return Some((out, Contrib::Remote));
    }
    if contains(&tx, &ty) && contains(&ty, &tb) {
        let mut out = local.clone();
        out.attrs = merge_attrs(base, local, remote);
        return Some((out, Contrib::Local));
    }

    // ── M3：两侧都是对 base 的纯追加（前缀相同）→ 拼接两侧追加内容。
    if tx != tb
        && ty != tb
        && tx.starts_with(tb.as_str())
        && ty.starts_with(tb.as_str())
    {
        let sx = &tx[tb.len()..];
        let sy = &ty[tb.len()..];
        let n = tb.chars().count();
        let prefix_src = if block_canonical(local) <= block_canonical(remote) {
            local
        } else {
            remote
        };
        let mut content = prefix_chars_inlines(&prefix_src.content, n);
        // 追加段先后由文本字典序决定 —— 与"谁是 local"无关。
        let (first, second) = if sx <= sy { (local, remote) } else { (remote, local) };
        content.extend(slice_inlines(&first.content, n));
        content.extend(slice_inlines(&second.content, n));
        content.retain(|i| !i.text.is_empty() || !i.marks.is_empty());
        let type_ = if local.type_ == remote.type_ {
            local.type_.clone()
        } else {
            prefix_src.type_.clone()
        };
        let out = Block {
            id: base.id.clone(),
            type_,
            attrs: merge_attrs(base, local, remote),
            content,
        };
        return Some((out, Contrib::Both));
    }

    // ── M4：checklist 一侧勾选、一侧改文本 → 文本取改动侧，勾选取改动侧。
    // （勾选状态本身由 `merge_attrs` 的单侧变更规则保住；两侧都改勾选时降级为
    //  "默认未勾选 + 另一状态留档"。）
    if (is_checklist(base) || is_checklist(local) || is_checklist(remote))
        && (tx == tb || ty == tb)
    {
        let winner = if tx == tb { remote } else { local };
        let mut out = winner.clone();
        out.attrs = merge_attrs(base, local, remote);
        return Some((
            out,
            if tx == tb {
                Contrib::Remote
            } else {
                Contrib::Local
            },
        ));
    }

    // ── M5：以上都不成立 → 该块判冲突，整篇走 §6。
    None
}

/// "带格式的一侧"：marks/attrs 更多者胜；平局比 canonical（对称、确定）。
fn richer<'a>(a: &'a Block, b: &'a Block) -> (&'a Block, Contrib) {
    let score = |blk: &Block| -> (usize, usize) {
        (blk.content.iter().map(|i| i.marks.len()).sum(), blk.attrs.len())
    };
    let (sa, sb) = (score(a), score(b));
    if sa > sb {
        (a, Contrib::Local)
    } else if sb > sa {
        (b, Contrib::Remote)
    } else if block_canonical(a) <= block_canonical(b) {
        (a, Contrib::Local)
    } else {
        (b, Contrib::Remote)
    }
}

fn is_checklist(b: &Block) -> bool {
    matches!(b.type_, BlockType::ChecklistItem)
}

/// attrs 三方合并：单侧变更采纳；两侧都改成不同值 → 取 canonical 小者，另一值留在
/// `conflict:<key>` 下（"属性冲突降级为审计 + 提示"，不升级为内容冲突；§4 原则）。
/// `checked` 例外：按 M4 默认未勾选。
fn merge_attrs(
    base: &Block,
    local: &Block,
    remote: &Block,
) -> BTreeMap<String, serde_json::Value> {
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for m in [&base.attrs, &local.attrs, &remote.attrs] {
        keys.extend(m.keys().cloned());
    }
    let mut out: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for k in keys {
        let vb = base.attrs.get(&k);
        let vl = local.attrs.get(&k);
        let vr = remote.attrs.get(&k);
        if vl == vr {
            if let Some(v) = vl {
                out.insert(k, v.clone());
            }
            continue;
        }
        if vl == vb {
            if let Some(v) = vr {
                out.insert(k, v.clone());
            }
            continue;
        }
        if vr == vb {
            if let Some(v) = vl {
                out.insert(k, v.clone());
            }
            continue;
        }
        match (vl, vr) {
            (Some(a), Some(c)) => {
                if k == "checked" {
                    out.insert("checked".into(), serde_json::json!(false));
                    out.insert(
                        "conflict:checked".into(),
                        serde_json::json!({ "a": a, "b": c }),
                    );
                } else {
                    let (win, lose) = if json_key(a) <= json_key(c) {
                        (a, c)
                    } else {
                        (c, a)
                    };
                    out.insert(k.clone(), win.clone());
                    out.insert(format!("conflict:{k}"), lose.clone());
                }
            }
            (Some(a), None) => {
                out.insert(k, a.clone());
            }
            (None, Some(c)) => {
                out.insert(k, c.clone());
            }
            (None, None) => {}
        }
    }
    out
}

fn json_key(v: &serde_json::Value) -> String {
    notera_core::canonical_json(v)
}

fn contains(hay: &str, needle: &str) -> bool {
    needle.is_empty() || hay.contains(needle)
}

/// 取行内序列的前 `n` 个码点（marks 跟着文本走）。
fn prefix_chars_inlines(inlines: &[Inline], n: usize) -> Vec<Inline> {
    let mut left = n;
    let mut out = Vec::new();
    for i in inlines {
        if left == 0 {
            break;
        }
        let c = i.text.chars().count();
        if c <= left {
            out.push(i.clone());
            left -= c;
        } else {
            out.push(Inline {
                text: i.text.chars().take(left).collect(),
                marks: marks_of(i),
            });
            left = 0;
        }
    }
    out
}

/// 丢掉行内序列的前 `n` 个码点（= 取纯追加那一段）。
fn slice_inlines(inlines: &[Inline], n: usize) -> Vec<Inline> {
    let mut left = n;
    let mut out = Vec::new();
    for i in inlines {
        let c = i.text.chars().count();
        if left >= c {
            left -= c;
            continue;
        }
        let t: String = i.text.chars().skip(left).collect();
        left = 0;
        if !t.is_empty() || !i.marks.is_empty() {
            out.push(Inline {
                text: t,
                marks: marks_of(i),
            });
        }
    }
    out
}

fn marks_of(i: &Inline) -> Vec<Mark> {
    i.marks.clone()
}

/// §3.4：合并产物过不了校验 → 丢弃产物，退回冲突（保留双方）。
fn conflict_for_invalid_merge(doc: &Document, base: &Document, _err: &RichError) -> MergeOutcome {
    let mut ids: Vec<String> = Vec::new();
    for blk in &doc.content {
        let unchanged = base
            .content
            .iter()
            .any(|x| x.id == blk.id && x.same_content(blk));
        if !unchanged && !ids.contains(&blk.id) {
            ids.push(blk.id.clone());
        }
    }
    if ids.is_empty() {
        ids = doc.content.iter().map(|x| x.id.clone()).collect();
    }
    MergeOutcome::Conflict {
        conflicting_block_ids: ids,
    }
}
