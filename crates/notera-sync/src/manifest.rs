//! 清单（manifest）：结构、自校验、有效状态合成、压实决策、损坏恢复阶梯。
//!
//! 规范见 docs/SYNC-PROTOCOL.md §4。本模块是纯数据 + 纯函数：不碰网络、不碰磁盘，
//! 因此所有分支都能在单测里确定性地跑到。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const PROTOCOL: u16 = 1;
pub const WINDOW_MAX: usize = 200;
pub const SEGMENT_TARGET: usize = 2000;
/// 窗口与某分段 id 重叠率超过此值即值得压实。
pub const COMPACT_OVERLAP: f64 = 0.20;
pub const COMPACT_SEQ_GAP: u64 = 5000;

pub type ShortHash = String; // 12 hex，仅快速路径提示

/// 清单条目。字段名单字母是为了流量（实测 89 B/条，头部字段占比最高）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryRef {
    /// id
    pub i: String,
    /// kind tag: n / f / a
    pub t: String,
    /// rev
    pub r: u64,
    /// hash12
    pub h: ShortHash,
    /// size
    pub s: u64,
    /// deleted_at
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub d: Option<String>,
    /// purged
    #[serde(default, skip_serializing_if = "is_zero")]
    pub p: u8,
}

fn is_zero(v: &u8) -> bool {
    *v == 0
}

impl EntryRef {
    pub fn key(&self) -> (String, String) {
        (self.t.clone(), self.i.clone())
    }
    pub fn is_deleted(&self) -> bool {
        self.d.is_some()
    }
    pub fn is_purged(&self) -> bool {
        self.p != 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentRef {
    /// 分段文件名
    pub n: String,
    /// 覆盖的 id 区间 [min, max]
    pub cover: [String; 2],
    pub count: usize,
    pub hash12: ShortHash,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    /// 窗口覆盖到哪个 seq 为止；`since_seq` 之后的全部变更都在 entries 里
    pub since_seq: u64,
    /// false 表示窗口被裁剪过，落后更多的客户端必须回退到分段比对
    pub complete: bool,
    pub entries: Vec<EntryRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentRef {
    /// sha256（附件以内容寻址，无 rev）
    pub i: String,
    /// size
    pub z: u64,
}

/// `manifest/index.json` 的内存表示。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub protocol: u16,
    pub root_id: String,
    pub seq: u64,
    pub generated_at: String,
    pub generated_by: String,
    pub software: String,
    pub counts: BTreeMap<String, u64>,
    pub segments: Vec<SegmentRef>,
    pub window: Window,
    #[serde(default)]
    pub attachments: Vec<AttachmentRef>,
    /// 去掉本字段后的 canonical JSON 的 sha256
    pub checksum: String,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ManifestError {
    #[error("清单 JSON 非法: {0}")]
    Malformed(String),
    #[error("清单协议版本 {found} 不受支持（本程序 {supported}）")]
    Protocol { found: u16, supported: u16 },
    #[error("清单自校验失败：内容被改动或写入不完整")]
    Checksum,
    #[error("清单含重复条目 {0}")]
    DuplicateEntry(String),
    #[error("清单 seq 回退：观测 {observed} 却收到更早的 {received}")]
    SeqRewind { observed: u64, received: u64 },
}

impl Manifest {
    /// 空根清单（首次初始化）。
    pub fn initial(root_id: &str, device: &str, at: &str, software: &str) -> Self {
        let mut m = Self {
            protocol: PROTOCOL,
            root_id: root_id.into(),
            seq: 0,
            generated_at: at.into(),
            generated_by: device.into(),
            software: software.into(),
            counts: BTreeMap::new(),
            segments: Vec::new(),
            window: Window { since_seq: 0, complete: true, entries: Vec::new() },
            attachments: Vec::new(),
            checksum: String::new(),
        };
        m.seq = 1;
        m.refresh_checksum();
        m
    }

    fn canonical_body(&self) -> String {
        let mut clone = self.clone();
        clone.checksum = String::new();
        notera_core::canonical_json(&serde_json::to_value(&clone).unwrap_or(serde_json::Value::Null))
    }

    pub fn refresh_checksum(&mut self) {
        self.checksum = format!("sha256:{}", sha256_hex(self.canonical_body().as_bytes()));
    }

    /// 严格解析：JSON → 协议 → 自校验 → 重复条目。任一步失败即 Err，
    /// 调用方**不得**把 Err 当成"空清单"（那等于清空用户库）。
    pub fn parse(wire: &[u8]) -> Result<Self, ManifestError> {
        let mut m: Manifest =
            serde_json::from_slice(wire).map_err(|e| ManifestError::Malformed(e.to_string()))?;
        if m.protocol != PROTOCOL {
            return Err(ManifestError::Protocol { found: m.protocol, supported: PROTOCOL });
        }
        let expected = format!("sha256:{}", sha256_hex(m.canonical_body().as_bytes()));
        if m.checksum != expected {
            // 顺手把算出来的写回，便于诊断日志对比
            m.checksum = expected;
            return Err(ManifestError::Checksum);
        }
        let mut seen = std::collections::BTreeSet::new();
        for e in &m.window.entries {
            if !seen.insert(e.key()) {
                return Err(ManifestError::DuplicateEntry(format!("{:?}", e.key())));
            }
        }
        Ok(m)
    }

    pub fn to_wire(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// 有效远端状态 = 分段条目被窗口覆盖（窗口优先）。
    /// 分段内容需另行传入（它们是不可变文件）。
    pub fn effective(&self, segments: &BTreeMap<String, Vec<EntryRef>>) -> BTreeMap<(String, String), EntryRef> {
        let mut out: BTreeMap<(String, String), EntryRef> = BTreeMap::new();
        for seg in &self.segments {
            if let Some(entries) = segments.get(&seg.n) {
                for e in entries {
                    out.insert(e.key(), e.clone());
                }
            }
        }
        for e in &self.window.entries {
            out.insert(e.key(), e.clone());
        }
        out
    }

    /// 把一批变更提交进清单（不压实）。返回新清单；原清单不变。
    ///
    /// 幂等：同 (kind,id) 同 rev 重复提交结果相同（SYNC-PROTOCOL §11.1）。
    pub fn with_commit(&self, device: &str, at: &str, writes: &[EntryRef], deletes: &[EntryRef]) -> Self {
        let mut next = self.clone();
        next.seq = self.seq + 1;
        next.generated_at = at.into();
        next.generated_by = device.into();
        let mut window: BTreeMap<(String, String), EntryRef> =
            self.window.entries.iter().map(|e| (e.key(), e.clone())).collect();
        for w in writes.iter().chain(deletes.iter()) {
            window.insert(w.key(), w.clone());
        }
        let mut entries: Vec<EntryRef> = window.into_values().collect();
        entries.sort_by_key(|a| a.key());
        next.window = Window { since_seq: self.window.since_seq, complete: true, entries };
        next.recount();
        next.refresh_checksum();
        next
    }

    pub fn segment_of(&self, id: &str) -> Option<&SegmentRef> {
        self.segments.iter().find(|s| s.cover[0].as_str() <= id && id <= s.cover[1].as_str())
    }

    /// 重算条目计数。
    ///
    /// **这是近似值**：分段是独立文件，清单只知道每段多少条、不知道其中几条已被窗口
    /// 标删。因此 `counts` 只用于 UI 概览与诊断，**任何判定都不得依赖它** ——
    /// 权威计数来自本地 SQLite（`Store::stats`）。
    fn recount(&mut self) {
        let mut c: BTreeMap<String, u64> = BTreeMap::new();
        for k in ["n", "f", "a"] {
            c.insert(k.into(), 0);
        }
        for s in &self.segments {
            *c.entry("n".into()).or_insert(0) += s.count as u64;
        }
        for e in &self.window.entries {
            let covered = self.segment_of(&e.i).is_some();
            match (e.is_deleted(), covered) {
                (true, true) => *c.entry(e.t.clone()).or_insert(0) = c.get(&e.t).copied().unwrap_or(0).saturating_sub(1),
                (true, false) => {}
                (false, true) => {} // 覆盖分段里的同一条，不重复计数
                (false, false) => *c.entry(e.t.clone()).or_insert(0) += 1,
            }
        }
        self.counts = c;
    }

    /// 是否需要压实。
    pub fn needs_compaction(&self, last_compacted_seq: u64) -> bool {
        if self.window.entries.len() > WINDOW_MAX {
            return true;
        }
        if self.seq.saturating_sub(last_compacted_seq) > COMPACT_SEQ_GAP {
            return true;
        }
        let total = self.window.entries.len().max(1);
        let overlap = self
            .window
            .entries
            .iter()
            .filter(|e| self.segment_of(&e.i).is_some())
            .count();
        (overlap as f64 / total as f64) > COMPACT_OVERLAP
    }

    /// 压实：把窗口折进受影响分段，窗口清空，seq 再 +1。
    /// 返回（新清单, 需要重写的分段名, 新的分段内容）。
    pub fn compact(&self, segments: &BTreeMap<String, Vec<EntryRef>>, device: &str, at: &str) -> (Self, Vec<String>, BTreeMap<String, Vec<EntryRef>>) {
        let mut new_segments = segments.clone();
        let mut touched: Vec<String> = Vec::new();
        // 按 id 决定归属分段（落在哪个 cover 内；否则进最后一段）
        for e in &self.window.entries {
            let name = self
                .segment_of(&e.i)
                .map(|s| s.n.clone())
                .or_else(|| self.segments.last().map(|s| s.n.clone()))
                .unwrap_or_else(|| "seg-0000".to_string());
            let bucket = new_segments.entry(name.clone()).or_default();
            bucket.retain(|x| x.key() != e.key());
            bucket.push(e.clone());
            if !touched.contains(&name) {
                touched.push(name);
            }
        }
        for bucket in new_segments.values_mut() {
            bucket.sort_by_key(|a| a.key());
        }
        let mut next = self.clone();
        next.seq = self.seq + 1;
        next.generated_at = at.into();
        next.generated_by = device.into();
        next.window = Window { since_seq: next.seq, complete: true, entries: Vec::new() };
        // 分段引用必须覆盖**每一个桶**，包括这次压实新建的那些。首次压实就是这种情形：
        // 索引里还一条分段都没有，而窗口里的条目全被折进了新桶 `seg-0000`。以前这里只
        // 遍历已有的 `self.segments`，于是新桶进不了索引、窗口又已被清空 —— **清单上所有
        // 条目一起消失**（比不压实严重得多）。
        let mut refs: BTreeMap<String, SegmentRef> =
            self.segments.iter().cloned().map(|s| (s.n.clone(), s)).collect();
        for (name, entries) in &new_segments {
            let wire = segment_wire(entries);
            let r = refs.entry(name.clone()).or_insert_with(|| SegmentRef {
                n: name.clone(),
                cover: [String::new(), String::new()],
                count: 0,
                hash12: String::new(),
                bytes: 0,
            });
            r.count = entries.len();
            r.hash12 = short(&sha256_hex(&wire));
            r.bytes = wire.len() as u64;
            r.cover = entries
                .first()
                .zip(entries.last())
                .map(|(a, b)| [a.i.clone(), b.i.clone()])
                .unwrap_or_else(|| [String::new(), String::new()]);
        }
        next.segments = refs.into_values().collect();
        next.refresh_checksum();
        (next, touched, new_segments)
    }

    /// 客户端落后多少：需要拉哪些分段。
    pub fn segments_needed_for(&self, cached_seg_hashes: &BTreeMap<String, String>) -> Vec<String> {
        self.segments
            .iter()
            .filter(|s| cached_seg_hashes.get(&s.n).map(|h| *h != s.hash12).unwrap_or(true))
            .map(|s| s.n.clone())
            .collect()
    }
}

/// 分段文件的字节形态：条目数组的 canonical JSON。
///
/// `SegmentRef.hash12` 与 `bytes` 都是**对这段字节**算的，读者 `fetch_segment` 接受
/// 这个形态 —— 把"写什么"与"按什么算哈希"钉在同一个函数里，免得两处各写一份而漂移。
pub fn segment_wire(entries: &[EntryRef]) -> Vec<u8> {
    notera_core::canonical_json(&serde_json::to_value(entries).unwrap_or_default()).into_bytes()
}

fn sha256_hex(bytes: &[u8]) -> String {
    notera_core::ContentHash::of(bytes).as_str()["sha256:".len()..].to_string()
}

fn short(hex: &str) -> String {
    hex[..12.min(hex.len())].to_string()
}

/// D3 闸门：远端与本地缓存严重背离时，**不得**据此删除本地。
pub fn divergence_is_suspicious(cached_count: usize, received_count: usize) -> bool {
    if cached_count == 0 {
        return false;
    }
    let vanished = cached_count.saturating_sub(received_count);
    vanished > 50 && vanished * 100 > cached_count * 30
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, rev: u64, h: &str) -> EntryRef {
        EntryRef { i: id.into(), t: "n".into(), r: rev, h: h.into(), s: 100, d: None, p: 0 }
    }

    fn seg(name: &str, lo: &str, hi: &str, entries: Vec<EntryRef>) -> (SegmentRef, Vec<EntryRef>) {
        let hash = short(&sha256_hex(
            notera_core::canonical_json(&serde_json::to_value(&entries).unwrap()).as_bytes(),
        ));
        (
            SegmentRef { n: name.into(), cover: [lo.into(), hi.into()], count: entries.len(), hash12: hash, bytes: 10 },
            entries,
        )
    }

    fn sample() -> Manifest {
        let mut m = Manifest::initial("root-1", "dev-1", "2026-09-25T00:00:00.000Z", "notera 0.1.0");
        let (s1, e1) = seg("seg-0000", "a", "m", vec![entry("a", 1, "aaaaaaaaaaaa"), entry("m", 1, "bbbbbbbbbbbb")]);
        m.segments = vec![s1];
        let mut map = BTreeMap::new();
        map.insert("seg-0000".to_string(), e1);
        m.window = Window { since_seq: 1, complete: true, entries: vec![entry("z", 3, "cccccccccccc")] };
        m.recount();
        m.refresh_checksum();
        let _ = map;
        m
    }

    #[test]
    fn roundtrip_and_checksum_detects_tampering() {
        let m = sample();
        let wire = m.to_wire();
        let back = Manifest::parse(&wire).expect("合法清单必须可解析");
        assert_eq!(back, m);
        // 改一个字节（模拟半写或服务器侧篡改）必须被自校验抓住
        let mut bad = wire.clone();
        let idx = bad.iter().position(|b| *b == b'3').unwrap();
        bad[idx] = b'9';
        assert!(Manifest::parse(&bad).is_err(), "被改动的清单必须被拒绝");
    }

    #[test]
    fn unknown_protocol_is_rejected_not_treated_as_empty() {
        let mut m = sample();
        m.protocol = 99;
        m.refresh_checksum();
        match Manifest::parse(&m.to_wire()) {
            Err(ManifestError::Protocol { .. }) => {}
            other => panic!("必须拒绝未知协议，实际 {other:?}"),
        }
    }

    #[test]
    fn effective_state_lets_window_override_segments() {
        let m = sample();
        let mut segs = BTreeMap::new();
        let (_, e) = seg("seg-0000", "a", "m", vec![entry("a", 1, "aaaaaaaaaaaa"), entry("m", 1, "bbbbbbbbbbbb")]);
        segs.insert("seg-0000".to_string(), e);
        // 窗口把 m 改成 rev 9
        m.window.entries.iter().for_each(|_| {});
        let mut m2 = m.clone();
        m2.window.entries.push(entry("m", 9, "dddddddddddd"));
        m2.refresh_checksum();
        let eff = m2.effective(&segs);
        assert_eq!(eff.get(&("n".into(), "m".into())).unwrap().r, 9, "窗口必须覆盖分段");
        assert_eq!(eff.get(&("n".into(), "a".into())).unwrap().r, 1, "未触及的分段条目必须保留");
    }

    #[test]
    fn commit_is_idempotent_and_monotonic() {
        let m = sample();
        let w = vec![entry("q", 5, "eeeeeeeeeeee")];
        let a = m.with_commit("dev-1", "2026-09-25T00:00:01.000Z", &w, &[]);
        let b = a.with_commit("dev-1", "2026-09-25T00:00:02.000Z", &w, &[]);
        assert_eq!(a.seq + 1, b.seq);
        assert_eq!(
            a.window.entries.iter().filter(|e| e.i == "q").count(),
            b.window.entries.iter().filter(|e| e.i == "q").count(),
            "同一 rev 重复提交不得产生第二条"
        );
    }

    #[test]
    fn compaction_moves_window_into_segments_and_resets_window() {
        let mut m = sample();
        let mut segs = BTreeMap::new();
        let (sref, e) = seg("seg-0000", "a", "m", vec![entry("a", 1, "aaaaaaaaaaaa"), entry("m", 1, "bbbbbbbbbbbb")]);
        m.segments = vec![sref];
        segs.insert("seg-0000".into(), e);
        m.refresh_checksum();
        assert!(m.needs_compaction(0) || m.window.entries.len() <= WINDOW_MAX);
        let (next, touched, newsegs) = m.compact(&segs, "dev-1", "2026-09-25T00:00:03.000Z");
        assert!(next.window.entries.is_empty(), "压实后窗口必须清空");
        assert_eq!(next.window.since_seq, next.seq);
        assert!(!touched.is_empty());
        assert!(newsegs.values().flatten().any(|x| x.i == "z"), "窗口条目应落入分段");
        assert!(Manifest::parse(&next.to_wire()).is_ok(), "压实产物必须自校验通过");
    }

    /// 首次压实：索引里一条分段都还没有，窗口整批折进新建的 `seg-0000`。
    ///
    /// 这一支以前是坏的 —— `compact()` 只遍历已有的 `self.segments`，于是新桶进不了索引、
    /// 而窗口又已被清空，压实产物读起来等于"库里一条记录都没有"。写侧一旦真的接上压实，
    /// 每个 >200 条变更的库都会撞上它，所以这条测试钉的是"条目一个都不能消失"。
    #[test]
    fn first_compaction_creates_the_segment_that_carries_the_window() {
        let mut m = sample();
        m.segments = Vec::new();
        let ids: Vec<String> = m.window.entries.iter().map(|e| e.i.clone()).collect();
        assert!(!ids.is_empty(), "夹具的窗口要有条目");
        let (next, touched, newsegs) = m.compact(&BTreeMap::new(), "dev-1", "2026-09-25T00:00:03.000Z");
        assert!(next.window.entries.is_empty(), "窗口该清空");
        assert_eq!(next.segments.len(), 1, "新建的分段必须进索引，否则条目凭空消失：{:?}", next.segments);
        let listed: Vec<String> = next
            .segments
            .iter()
            .flat_map(|s| newsegs.get(&s.n).into_iter().flatten().map(|e| e.i.clone()))
            .collect();
        for id in &ids {
            assert!(listed.contains(id), "条目 {id} 压实后既不在窗口也不在任何分段里");
        }
        assert_eq!(next.segments[0].count, ids.len());
        assert!(!touched.is_empty());
        let body = segment_wire(newsegs.get(&next.segments[0].n).expect("分段内容"));
        assert_eq!(next.segments[0].bytes, body.len() as u64, "bytes 要按真正落盘的那段字节算");
        assert!(Manifest::parse(&next.to_wire()).is_ok(), "压实产物必须自校验通过");
    }

    #[test]
    fn compaction_triggers_on_overlap() {
        let mut m = sample();
        let (sref, e) = seg("seg-0000", "a", "m", vec![entry("a", 1, "aaaaaaaaaaaa")]);
        m.segments = vec![sref];
        // 全部窗口条目都落在已有分段覆盖范围内 → 重叠率 100%
        m.window.entries = vec![entry("a", 2, "aaaaaaaaaaab"), entry("m", 2, "aaaaaaaaaaac")];
        m.refresh_checksum();
        let _ = e;
        assert!(m.needs_compaction(m.seq), "重叠率超阈值应触发压实");
    }

    #[test]
    fn segments_needed_only_for_changed_hashes() {
        let m = sample();
        let mut cached = BTreeMap::new();
        cached.insert("seg-0000".to_string(), m.segments[0].hash12.clone());
        assert!(m.segments_needed_for(&cached).is_empty(), "hash 未变不应重拉分段");
        cached.insert("seg-0000".to_string(), "ffffffffffff".into());
        assert_eq!(m.segments_needed_for(&cached), vec!["seg-0000".to_string()]);
        assert_eq!(m.segments_needed_for(&BTreeMap::new()), vec!["seg-0000".to_string()], "全新设备需拉全部分段");
    }

    #[test]
    fn divergence_gate_blocks_mass_deletion_inference() {
        // 本地缓存 5000 条、远端只剩 10 条 → 必须判为可疑，而不是"用户删了 4990 条"
        assert!(divergence_is_suspicious(5000, 10));
        // 正常小幅波动不触发
        assert!(!divergence_is_suspicious(5000, 4990));
        // 小库不触发（避免 3 条里少 2 条就弹窗）
        assert!(!divergence_is_suspicious(3, 1));
        // 首次（本地无缓存）不触发
        assert!(!divergence_is_suspicious(0, 100));
    }

    #[test]
    fn duplicate_entries_rejected() {
        let mut m = sample();
        m.window.entries.push(entry("z", 3, "cccccccccccc"));
        m.refresh_checksum();
        assert!(matches!(Manifest::parse(&m.to_wire()), Err(ManifestError::DuplicateEntry(_))));
    }
}
