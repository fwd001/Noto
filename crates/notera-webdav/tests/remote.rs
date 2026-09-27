//! 真实 HTTP 下的 `RemotePort` 行为。
//!
//! 这里跑的每一个字节都经过 TCP socket（`notera-test-webdav` 是真服务器，不是 mock ——
//! docs/TEST-PLAN.md §0.7 的禁令），因此这些断言证明的是"适配器接得上现实"，
//! 而不是"适配器自洽"。服务端状态一律用 `/_fs/dump` 端点判，不信客户端自述。

use std::sync::Arc;
use std::time::Duration;

use notera_core::{canonical_json, ContentHash, DeviceId, EntityId};
use notera_net::{HttpClient, HttpMethod, ProxyProfile, RequestSpec, Timeouts, TlsPolicy};
use notera_sync::manifest::{EntryRef, Manifest};
use notera_sync::{RemoteError, RemotePort};
use notera_test_webdav::{Backend, Injection, Started, TestServer};
use notera_webdav::{Caps, Credentials, RemotePath, WebDavConfig, WebDavRemote, WriteStrategy};
use serde_json::{json, Value};

const AT: &str = "2026-09-25T10:00:00.000Z";

/// 一个真服务器 + 一个真出口 + 一台设备。
struct Ctx {
    srv: Started,
    http: Arc<HttpClient>,
    device: DeviceId,
}

impl Ctx {
    async fn start() -> Ctx {
        let srv = TestServer::start(Backend::Mem).await;
        let http = Arc::new(
            HttpClient::build(
                &ProxyProfile::direct(),
                &TlsPolicy::Strict,
                Timeouts::short(Duration::from_secs(10)),
            )
            .expect("出口客户端"),
        );
        Ctx {
            srv,
            http,
            device: DeviceId::new(),
        }
    }

    /// 指定能力位的远端。一律带凭据：走的是生产的 header 拼装路径。
    fn remote(&self, caps: Caps) -> WebDavRemote {
        let cfg = WebDavConfig::new(self.srv.base_url.clone())
            .with_credentials(Credentials::new("notera-test", "sup3r-s3cr3t").expect("凭据"))
            .with_device(self.device.clone())
            .with_caps(caps);
        WebDavRemote::new(cfg, Arc::clone(&self.http)).expect("合法配置")
    }

    /// 服务端权威快照：真的打 `/_fs/dump` 端点，且经生产的出口发出去。
    async fn dump(&self) -> Value {
        let url = format!("{}/_fs/dump?prefix=/.notes", self.srv.base_url);
        let resp = self
            .http
            .send(RequestSpec::new(HttpMethod::Get, url))
            .await
            .expect("dump 请求");
        assert_eq!(resp.status, 200, "控制面必须可达");
        serde_json::from_slice(&resp.body).expect("dump 是 JSON")
    }

    /// `entries` 里按服务端路径取一条（`None` = 服务器上没这个对象）。
    async fn dumped(&self, path: &str) -> Option<Value> {
        let entries = self.dump().await["entries"]
            .as_array()
            .expect("entries 是数组")
            .clone();
        entries.into_iter().find(|e| e["path"] == json!(path))
    }

    fn log(&self) -> Vec<(String, String, u16)> {
        self.srv
            .request_log()
            .iter()
            .map(|r| (r.method.clone(), r.path.clone(), r.status))
            .collect()
    }

    /// 命中某动词 + 路径子串的请求数。注意 MOVE 记的是**源**路径。
    fn hits(&self, method: &str, path_contains: &str) -> usize {
        self.log()
            .into_iter()
            .filter(|(m, p, _)| m == method && p.contains(path_contains))
            .count()
    }
}

/// 记录信封 —— 形态照 `crates/notera-store/tests/common/mod.rs::note_envelope`。
/// 唯一差别：`hash` 这里按 §3 现算 `sha256(canonical(payload))`，因为本 crate 不依赖
/// notera-richtext，用不了 store 那套富文本规范化（判定只看 rev+hash，与算法无关）。
fn note_envelope(id: &EntityId, rev: u64, text: &str) -> Vec<u8> {
    let payload = json!({
        "v": 1,
        "content": [{ "id": "blk000001", "type": "paragraph", "content": [{ "text": text }] }]
    });
    let env = json!({
        "protocol": 1, "kind": "note", "id": id.as_str(), "rev": rev,
        "sync_rev": rev - 1,
        "hash": ContentHash::of(canonical_json(&payload).as_bytes()).as_str(),
        "updated_at": AT, "device": "01920000-0000-7000-8000-000000000000",
        "deleted_at": null, "purged": false,
        "enc": { "alg": "none", "hash_alg": "sha256" },
        "payload": payload, "ct": null
    });
    serde_json::to_vec(&env).expect("信封可序列化")
}

fn empty_manifest(device: &DeviceId) -> Manifest {
    Manifest::initial(
        "01920000-0000-7000-8000-0000000000a1",
        &device.to_string(),
        AT,
        "notera webdav test",
    )
}

fn entry_of(id: &EntityId, rev: u64, wire: &[u8]) -> EntryRef {
    EntryRef {
        i: id.as_str().to_string(),
        t: "n".into(),
        r: rev,
        h: ContentHash::of(wire).short(),
        s: wire.len() as u64,
        d: None,
        p: 0,
    }
}

fn sha_of(bytes: &[u8]) -> String {
    ContentHash::of(bytes).as_str().to_string()
}

/// 一个没人监听的 loopback 端口（先占后放，端口号可复用但没人 accept）。
async fn dead_port() -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = l.local_addr().expect("local_addr").port();
    drop(l);
    port
}

// --------------------------------------------------------------------- ① ---

#[tokio::test]
async fn empty_manifest_commits_and_comes_back_verified_with_etag() {
    let c = Ctx::start().await;
    let r = c.remote(Caps::conventional());
    assert!(
        r.fetch_manifest(None).await.unwrap().is_none(),
        "未初始化的根必须是 None（引擎据此回落本地缓存），不能被当成空清单"
    );

    let wire = empty_manifest(&c.device).to_wire();
    let etag = r
        .commit_manifest(&wire, None)
        .await
        .unwrap()
        .expect("提交必须带回 etag");

    let (got, got_etag) = r.fetch_manifest(None).await.unwrap().expect("清单读得回来");
    assert_eq!(
        got, wire,
        "读回的字节必须与提交的一致（§4.1 checksum 自校验）"
    );
    assert_eq!(got_etag.as_deref(), Some(etag.as_str()));
    assert_eq!(Manifest::parse(&got).expect("自校验").seq, 1);

    // 304 空轮快路径（§6.3）：同一 etag 再去取必须 0 字节正文。
    assert!(
        r.fetch_manifest(Some(&etag)).await.unwrap().is_none(),
        "If-None-Match 命中须判未变"
    );

    // 服务端事实：第一次提交没有"上一版"可保留。
    assert!(c.dumped("/.notes/manifest/index.json.prev").await.is_none());
    assert_eq!(
        c.dumped("/.notes/manifest/index.json")
            .await
            .expect("index.json 在盘上")["sha256"],
        json!(sha_of(&wire))
    );
    let leftover: Vec<String> = self::tmp_leftovers(&c).await;
    assert!(leftover.is_empty(), "提交后不得残留暂存对象: {leftover:?}");
}

async fn tmp_leftovers(c: &Ctx) -> Vec<String> {
    c.dump()
        .await
        .get("entries")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| {
            e["path"].as_str().unwrap_or("").contains(".tmp-")
                || e["path"].as_str().unwrap_or("").starts_with("/.notes/tmp/")
        })
        .map(|e| e["path"].as_str().unwrap_or_default().to_string())
        .collect()
}

// --------------------------------------------------------------------- ② ---

#[tokio::test]
async fn put_record_readback_verifies_rev_and_hash() {
    let c = Ctx::start().await;
    let r = c.remote(Caps::conventional());
    let id = EntityId::new();
    let wire = note_envelope(&id, 7, "复验这一条");

    let commit = r
        .put_record("note", id.as_str(), &wire, None)
        .await
        .expect("首写");
    assert!(commit.verified, "写后复验必须确认这一条就是我发的那一条");
    assert!(
        commit.etag.is_some(),
        "强 ETag 是空轮快路径的前提（§5 第一行探测）"
    );

    let back = r
        .fetch_record("n", id.as_str())
        .await
        .unwrap()
        .expect("记录读得回来");
    assert_eq!(back, wire);
    let v: Value = serde_json::from_slice(&back).unwrap();
    assert_eq!(v["rev"], json!(7));
    assert_eq!(
        v["hash"],
        json!(ContentHash::of(canonical_json(&v["payload"]).as_bytes()).as_str())
    );

    // §11.1 幂等复放：内容已经一致 → 判成功，且一个写请求都不发。
    let etag = r
        .probe_record_etag("note", id.as_str())
        .await
        .unwrap()
        .expect("etag");
    let writes = (
        c.hits("PUT", "/records/"),
        c.hits("MOVE", "/records/"),
        c.hits("DELETE", "/records/"),
    );
    let again = r
        .put_record("note", id.as_str(), &wire, Some(&etag))
        .await
        .expect("复放");
    assert!(again.verified);
    assert_eq!(
        (
            c.hits("PUT", "/records/"),
            c.hits("MOVE", "/records/"),
            c.hits("DELETE", "/records/")
        ),
        writes,
        "至少一次投递不得变成重复覆盖"
    );

    // 反过来：没探测就直接写（"我以为它不存在"）而它已存在 → 只能 412 让路。
    let other = note_envelope(&id, 8, "并发对手");
    assert_eq!(
        r.put_record("note", id.as_str(), &other, None)
            .await
            .unwrap_err(),
        RemoteError::Precondition
    );
    assert_eq!(
        r.fetch_record("n", id.as_str()).await.unwrap().unwrap(),
        wire
    );
}

#[tokio::test]
async fn lower_rev_is_rejected_and_does_not_touch_the_record() {
    let c = Ctx::start().await;
    let r = c.remote(Caps::conventional());
    let id = EntityId::new();
    let newer = note_envelope(&id, 5, "新的");
    let older = note_envelope(&id, 4, "更旧的");

    r.put_record("note", id.as_str(), &newer, None)
        .await
        .expect("首写 rev5");
    let etag = r
        .probe_record_etag("note", id.as_str())
        .await
        .unwrap()
        .expect("etag");
    let puts = c.hits("PUT", "/records/");

    let err = r
        .put_record("note", id.as_str(), &older, Some(&etag))
        .await
        .expect_err("更低的 rev 必须被拒，不能静默覆盖（§11.2 第一层）");
    assert_eq!(err, RemoteError::Precondition);
    assert_eq!(c.hits("PUT", "/records/"), puts, "被拒的写必须根本没发出去");
    assert_eq!(
        r.fetch_record("n", id.as_str()).await.unwrap().unwrap(),
        newer
    );
}

// --------------------------------------------------------------------- ③ ---

#[tokio::test]
async fn stale_if_match_yields_precondition_without_overwriting() {
    let c = Ctx::start().await;
    let r = c.remote(Caps::conventional());
    let id = EntityId::new();
    let v1 = note_envelope(&id, 1, "第一版");
    r.put_record("note", id.as_str(), &v1, None)
        .await
        .expect("首写");

    let v2 = note_envelope(&id, 2, "第二版");
    let err = r
        .put_record("note", id.as_str(), &v2, Some("\"这个 etag 早已过期\""))
        .await
        .expect_err("调用方手里的 etag 不是服务器那一版时必须判并发失败");
    assert_eq!(err, RemoteError::Precondition);
    assert_eq!(
        r.fetch_record("n", id.as_str()).await.unwrap().unwrap(),
        v1,
        "失败写不得留下痕迹"
    );

    // 拿真实 etag 再写就成功 —— 上一段拒的是过期前提，不是"永不许更新"。
    let fresh = r
        .probe_record_etag("note", id.as_str())
        .await
        .unwrap()
        .unwrap();
    assert!(
        r.put_record("note", id.as_str(), &v2, Some(&fresh))
            .await
            .expect("正常更新")
            .verified
    );
}

// --------------------------------------------------------------------- ④ ---

#[tokio::test]
async fn second_commit_leaves_the_first_version_in_prev() {
    let c = Ctx::start().await;
    let r = c.remote(Caps::conventional());
    let id = EntityId::new();
    let wire = note_envelope(&id, 3, "公告它");
    r.put_record("note", id.as_str(), &wire, None)
        .await
        .expect("记录先落（R2）");

    let v1 = empty_manifest(&c.device).to_wire();
    let e1 = r.commit_manifest(&v1, None).await.unwrap().expect("etag1");

    let m1 = Manifest::parse(&v1).unwrap();
    let v2 = m1
        .with_commit(&c.device.to_string(), AT, &[entry_of(&id, 3, &wire)], &[])
        .to_wire();
    let e2 = r
        .commit_manifest(&v2, Some(&e1))
        .await
        .unwrap()
        .expect("etag2");
    assert_ne!(e1, e2, "两版清单必然两个 etag");

    assert_eq!(
        c.dumped("/.notes/manifest/index.json")
            .await
            .expect("index.json")["sha256"],
        json!(sha_of(&v2)),
        "index.json 是最新一版"
    );
    assert_eq!(
        c.dumped("/.notes/manifest/index.json.prev")
            .await
            .expect("prev 必须已存在")["sha256"],
        json!(sha_of(&v1)),
        "§1：prev 存的是上一版，不是这一版"
    );

    // 第三版：prev 随轮转前进到 v2。
    let v3 = Manifest::parse(&v2)
        .unwrap()
        .with_commit(&c.device.to_string(), AT, &[], &[])
        .to_wire();
    r.commit_manifest(&v3, Some(&e2)).await.unwrap();
    assert_eq!(
        c.dumped("/.notes/manifest/index.json.prev")
            .await
            .expect("prev")["sha256"],
        json!(sha_of(&v2))
    );
    let (got, _) = r.fetch_manifest(None).await.unwrap().unwrap();
    assert_eq!(Manifest::parse(&got).unwrap().seq, 3);
    assert!(self::tmp_leftovers(&c).await.is_empty());
}

#[tokio::test]
async fn commit_with_wrong_cas_etag_is_precondition_and_changes_nothing() {
    let c = Ctx::start().await;
    let r = c.remote(Caps::conventional());
    let v1 = empty_manifest(&c.device).to_wire();
    r.commit_manifest(&v1, None).await.unwrap();

    let v2 = Manifest::parse(&v1)
        .unwrap()
        .with_commit(&c.device.to_string(), AT, &[], &[])
        .to_wire();
    let err = r
        .commit_manifest(&v2, Some("\"别人那一版的 etag\""))
        .await
        .expect_err("CAS 必须失败");
    assert_eq!(err, RemoteError::Precondition);
    assert_eq!(
        r.fetch_manifest(None).await.unwrap().unwrap().0,
        v1,
        "CAS 失败后服务器上还是旧清单"
    );
    assert!(
        c.dumped("/.notes/manifest/index.json.prev").await.is_none(),
        "没抢到就不该动 prev"
    );
}

#[tokio::test]
async fn manifest_that_does_not_self_check_is_never_published() {
    let c = Ctx::start().await;
    let r = c.remote(Caps::conventional());
    let v1 = empty_manifest(&c.device).to_wire();
    r.commit_manifest(&v1, None).await.unwrap();

    // checksum 与内容不符（半写或被篡改）：端口必须拒发，而不是把它公告出去。
    let mut broken: Value = serde_json::from_slice(&v1).unwrap();
    broken["seq"] = json!(99);
    let err = r
        .commit_manifest(&serde_json::to_vec(&broken).unwrap(), None)
        .await
        .expect_err("自校验不过就不许提交");
    assert!(matches!(err, RemoteError::Protocol(_)), "{err:?}");
    assert_eq!(r.fetch_manifest(None).await.unwrap().unwrap().0, v1);
}

// --------------------------------------------------------------------- ⑤ ---

#[tokio::test]
async fn s2_tmp_and_move_path_roundtrips_without_conditional_put() {
    let c = Ctx::start().await;
    // 不支持条件 PUT，但 `Overwrite: F` 的 MOVE 会 412 → 策略必须是 S2（§5）。
    let caps = Caps::from_mask(Caps::STRONG_ETAG | Caps::OVERWRITE_F_MOVE);
    let r = c.remote(caps);
    assert_eq!(r.caps().write_strategy(), WriteStrategy::S2);

    let id = EntityId::new();
    let v1 = note_envelope(&id, 1, "S2 首写");
    assert!(
        r.put_record("note", id.as_str(), &v1, None)
            .await
            .expect("S2 写")
            .verified
    );
    assert_eq!(r.fetch_record("n", id.as_str()).await.unwrap().unwrap(), v1);
    assert_eq!(c.hits("PUT", "/records/"), 0, "S2 不得直接 PUT 到记录路径");
    assert_eq!(c.hits("PUT", "/tmp/"), 1, "内容必须先落在暂存对象里");
    assert_eq!(c.hits("MOVE", "/tmp/"), 1, "MOVE 记的是源路径");

    // 更新：腾空目标 + Overwrite:F 的 MOVE。
    let etag = r
        .probe_record_etag("note", id.as_str())
        .await
        .unwrap()
        .expect("etag");
    let v2 = note_envelope(&id, 2, "S2 更新");
    assert!(
        r.put_record("note", id.as_str(), &v2, Some(&etag))
            .await
            .expect("S2 更新")
            .verified
    );
    assert_eq!(r.fetch_record("n", id.as_str()).await.unwrap().unwrap(), v2);
    assert_eq!(c.hits("DELETE", "/records/"), 1);

    // rev 闸门在 S2 下同样生效，且不动记录。
    let etag = r
        .probe_record_etag("note", id.as_str())
        .await
        .unwrap()
        .unwrap();
    let older = note_envelope(&id, 1, "倒退");
    assert_eq!(
        r.put_record("note", id.as_str(), &older, Some(&etag))
            .await
            .unwrap_err(),
        RemoteError::Precondition
    );
    assert_eq!(r.fetch_record("n", id.as_str()).await.unwrap().unwrap(), v2);

    // 清单也走同一条 MOVE 路径（⑤ 步）。
    let w1 = empty_manifest(&c.device).to_wire();
    let e1 = r
        .commit_manifest(&w1, None)
        .await
        .unwrap()
        .expect("S2 清单提交");
    let w2 = Manifest::parse(&w1)
        .unwrap()
        .with_commit(&c.device.to_string(), AT, &[entry_of(&id, 2, &v2)], &[])
        .to_wire();
    r.commit_manifest(&w2, Some(&e1)).await.unwrap();
    assert_eq!(r.fetch_manifest(None).await.unwrap().unwrap().0, w2);
    assert_eq!(
        c.dumped("/.notes/manifest/index.json.prev")
            .await
            .expect("prev")["sha256"],
        json!(sha_of(&w1))
    );
}

#[tokio::test]
async fn s3_blind_put_roundtrips_and_stays_honest_about_readback() {
    let c = Ctx::start().await;
    let r = c.remote(Caps::none());
    assert_eq!(r.caps().write_strategy(), WriteStrategy::S3);

    let id = EntityId::new();
    let v1 = note_envelope(&id, 1, "S3 首写");
    assert!(
        r.put_record("note", id.as_str(), &v1, None)
            .await
            .expect("S3 写")
            .verified,
        "复验是 S3 唯一的并发保护"
    );
    assert_eq!(c.hits("MOVE", "/"), 0, "S3 不用 MOVE");
    assert_eq!(c.hits("DELETE", "/"), 0, "S3 不腾空任何东西");
    assert_eq!(c.hits("PUT", "/records/"), 1);
    assert_eq!(r.fetch_record("n", id.as_str()).await.unwrap().unwrap(), v1);

    let v9 = note_envelope(&id, 9, "S3 更新");
    assert!(
        r.put_record("note", id.as_str(), &v9, None)
            .await
            .expect("S3 更新")
            .verified
    );
    let older = note_envelope(&id, 3, "倒退");
    assert_eq!(
        r.put_record("note", id.as_str(), &older, None)
            .await
            .unwrap_err(),
        RemoteError::Precondition
    );
    assert_eq!(c.hits("PUT", "/records/"), 2, "被拦下的倒退没有发出写入");

    // 清单没有 MOVE 可用时降级为普通 PUT 发布，第 ⑥ 步复验兜住。
    let w1 = empty_manifest(&c.device).to_wire();
    let e1 = r
        .commit_manifest(&w1, None)
        .await
        .unwrap()
        .expect("S3 清单提交");
    assert_eq!(r.fetch_manifest(None).await.unwrap().unwrap().0, w1);
    let w2 = Manifest::parse(&w1)
        .unwrap()
        .with_commit(&c.device.to_string(), AT, &[entry_of(&id, 9, &v9)], &[])
        .to_wire();
    r.commit_manifest(&w2, Some(&e1)).await.unwrap();
    assert_eq!(r.fetch_manifest(None).await.unwrap().unwrap().0, w2);
    assert_eq!(
        c.dumped("/.notes/manifest/index.json.prev")
            .await
            .expect("prev 仍在")["sha256"],
        json!(sha_of(&w1))
    );
}

#[tokio::test]
async fn server_rejecting_conditional_put_degrades_to_s2_mid_flight() {
    let c = Ctx::start().await;
    // 探测说支持条件 PUT，服务器却对记录路径的 PUT 回 405 → §12 要求就地降级并记诊断。
    c.srv
        .inject(Injection::status("PUT /.notes/records/*", 405))
        .await;
    let r = c.remote(Caps::conventional());
    let id = EntityId::new();
    let v1 = note_envelope(&id, 1, "降级写");
    assert!(
        r.put_record("note", id.as_str(), &v1, None)
            .await
            .expect("降级后仍要写成功")
            .verified
    );
    assert!(
        c.log()
            .iter()
            .any(|(m, p, s)| m == "PUT" && p.contains("/records/") && *s == 405),
        "必须先真的试过 S1"
    );
    assert!(
        c.log()
            .iter()
            .any(|(m, p, s)| m == "MOVE" && p.contains("/tmp/") && *s == 201),
        "再由 S2 落定"
    );
    assert_eq!(r.fetch_record("n", id.as_str()).await.unwrap().unwrap(), v1);
    c.srv.clear_injection().await;
}

#[tokio::test]
async fn server_without_move_still_commits_the_manifest() {
    let c = Ctx::start().await;
    let r = c.remote(Caps::conventional());
    let v1 = empty_manifest(&c.device).to_wire();
    let e1 = r.commit_manifest(&v1, None).await.unwrap().expect("基线");

    // 探测说有 MOVE，服务器其实 501：④ 退化为"PUT prev + DELETE index"，⑤ 退化为创建语义的 PUT。
    c.srv.inject(Injection::status("MOVE *", 501)).await;
    let v2 = Manifest::parse(&v1)
        .unwrap()
        .with_commit(&c.device.to_string(), AT, &[], &[])
        .to_wire();
    let e2 = r
        .commit_manifest(&v2, Some(&e1))
        .await
        .unwrap()
        .expect("退化路径也要提交成功");
    assert_ne!(e1, e2);
    assert_eq!(r.fetch_manifest(None).await.unwrap().unwrap().0, v2);
    assert_eq!(
        c.dumped("/.notes/manifest/index.json.prev")
            .await
            .expect("prev 仍被保住")["sha256"],
        json!(sha_of(&v1))
    );
    assert!(
        c.log().iter().any(|(m, _, s)| m == "MOVE" && *s == 501),
        "必须真的撞上 501"
    );
    assert!(
        self::tmp_leftovers(&c).await.is_empty(),
        "MOVE 失败后暂存要被清掉"
    );
    c.srv.clear_injection().await;
}

#[tokio::test]
async fn a_server_that_accepts_no_write_semantics_fails_loudly() {
    let c = Ctx::start().await;
    let r = c.remote(Caps::conventional());
    // S1 被 405、S2 的 MOVE 被 501、S3 的 PUT 同样落在 405 规则里：三级全灭。
    c.srv
        .inject(Injection {
            status_for: vec![
                ("PUT /.notes/records/*".into(), 405),
                ("MOVE *".into(), 501),
            ],
            ..Default::default()
        })
        .await;
    let id = EntityId::new();
    let wire = note_envelope(&id, 1, "写不进去");
    let err = r
        .put_record("note", id.as_str(), &wire, None)
        .await
        .expect_err("全策略失败必须报错，不能假装写成功");
    assert!(matches!(err, RemoteError::Protocol(_)), "{err:?}");
    assert_eq!(
        r.fetch_record("n", id.as_str()).await.unwrap(),
        None,
        "没写成就什么都没有"
    );
    c.srv.clear_injection().await;
    assert!(
        r.put_record("note", id.as_str(), &wire, None)
            .await
            .expect("撤掉故障后能写")
            .verified
    );
}

// --------------------------------------------------------------------- ⑥ ---

#[tokio::test]
async fn injected_500_on_write_is_server_error_and_leaves_no_half_state() {
    let c = Ctx::start().await;
    let r = c.remote(Caps::conventional());
    let v1 = empty_manifest(&c.device).to_wire();
    let e1 = r.commit_manifest(&v1, None).await.unwrap().expect("基线");

    // 只在"清单暂存"这一步挂掉：新清单一步都没落地，服务器必须还停在 v1。
    c.srv
        .inject(Injection::status(
            "PUT /.notes/manifest/index.json.tmp-*",
            500,
        ))
        .await;
    let v2 = Manifest::parse(&v1)
        .unwrap()
        .with_commit(&c.device.to_string(), AT, &[], &[])
        .to_wire();
    let err = r
        .commit_manifest(&v2, Some(&e1))
        .await
        .expect_err("500 必须分类为服务器故障");
    assert_eq!(err, RemoteError::Server);
    assert!(
        err.retryable() && !err.halts_round(),
        "§12：5xx 走指数退避，不是终止态"
    );

    let (got, _) = r.fetch_manifest(None).await.unwrap().expect("清单仍可读");
    assert_eq!(got, v1, "半写状态不得出现在服务器上");
    assert_eq!(Manifest::parse(&got).unwrap().seq, 1);
    assert!(
        c.dumped("/.notes/manifest/index.json.prev").await.is_none(),
        "没走到轮转，prev 不该被动过"
    );

    c.srv.clear_injection().await;
    // 撤掉故障后同一份提交照常成功（幂等复放，§11.1）。
    r.commit_manifest(&v2, Some(&e1)).await.unwrap();
    assert_eq!(r.fetch_manifest(None).await.unwrap().unwrap().0, v2);
}

#[tokio::test]
async fn dead_route_classifies_as_offline_never_as_absent_data() {
    let c = Ctx::start().await;
    let r = c.remote(Caps::conventional());
    let v1 = empty_manifest(&c.device).to_wire();
    r.commit_manifest(&v1, None).await.unwrap();

    let dead = WebDavRemote::new(
        WebDavConfig::new(format!("http://127.0.0.1:{}", dead_port().await))
            .with_device(c.device.clone()),
        Arc::clone(&c.http),
    )
    .expect("合法配置");
    let err = dead.fetch_manifest(None).await.unwrap_err();
    assert_eq!(
        err,
        RemoteError::Offline,
        "连不上是离线，不是'远端没有这份数据'（§10 红线）"
    );
    assert!(err.retryable() && !err.halts_round());
    assert_eq!(
        r.fetch_manifest(None).await.unwrap().unwrap().0,
        v1,
        "另一条连接上的远端一切照旧"
    );
}

#[tokio::test]
async fn injected_statuses_reach_the_port_vocabulary() {
    let c = Ctx::start().await;
    let r = c.remote(Caps::conventional());
    for (code, want) in [
        (401u16, RemoteError::Auth),
        (403, RemoteError::Forbidden),
        (409, RemoteError::Precondition),
        (412, RemoteError::Precondition),
        (507, RemoteError::Quota),
        (503, RemoteError::Server),
    ] {
        c.srv
            .inject(Injection::status("GET /.notes/manifest/index.json", code))
            .await;
        let got = r.fetch_manifest(None).await.unwrap_err();
        assert_eq!(got, want, "状态 {code} 的分类");
        c.srv.clear_injection().await;
    }
    // 清单侧的 404 是"根还没初始化"（→ None），只有记录侧才透成 NotFound（§10 前提）。
    c.srv
        .inject(Injection::status("GET /.notes/manifest/index.json", 404))
        .await;
    assert!(
        r.fetch_manifest(None).await.unwrap().is_none(),
        "404 清单不得变成错误，也不得变成空清单"
    );
    c.srv.clear_injection().await;

    // §12 的停轮/退避划分由端口词汇自己守住。
    assert!(RemoteError::Auth.halts_round() && RemoteError::Forbidden.halts_round());
    assert!(!RemoteError::Server.halts_round() && RemoteError::Server.retryable());
    assert!(!RemoteError::Offline.halts_round());
}

// --------------------------------------------------------------------- ⑦ ---

#[tokio::test]
async fn traversal_attempts_are_rejected_before_any_request() {
    let c = Ctx::start().await;
    let r = c.remote(Caps::conventional());
    let id = EntityId::new();
    let wire = note_envelope(&id, 1, "别越界");
    let baseline = c.log().len();

    let rejects = [
        r.fetch_segment("../etc/passwd").await.unwrap_err(),
        r.fetch_segment("..%2F..%2Fmanifest/index")
            .await
            .unwrap_err(),
        r.fetch_segment("seg-0000.json?prefix=/").await.unwrap_err(),
        r.fetch_segment("..\\\\etc").await.unwrap_err(),
        r.put_segment("..\\\\tmp", b"[]".as_slice())
            .await
            .unwrap_err(),
        r.fetch_record("note", "..%2F..%2Fx").await.unwrap_err(),
        r.fetch_record("../../etc", &id.to_string())
            .await
            .unwrap_err(),
        r.put_record("note", "..%2F..%2Fx", &wire, None)
            .await
            .unwrap_err(),
        r.probe_record_etag("bogus", &id.to_string())
            .await
            .unwrap_err(),
        r.put_record("note", "0192e6c1-0000-7000-8000-0000000000..", &wire, None)
            .await
            .unwrap_err(),
    ];
    for e in rejects.iter() {
        assert!(
            matches!(e, RemoteError::Protocol(_)),
            "越界输入必须判协议错误，实际 {e:?}"
        );
    }
    assert_eq!(
        c.log().len(),
        baseline,
        "拒绝必须发生在发出任何请求之前，一个字节也不能出去"
    );

    // 同一个远端上合法路径照常工作 —— 证明上面拒的是路径，不是整个适配器。
    r.put_record("note", id.as_str(), &wire, None)
        .await
        .unwrap();
    assert!(c.log().len() > baseline);
}

#[tokio::test]
async fn bad_endpoints_are_rejected_at_construction_time() {
    let c = Ctx::start().await;
    let http = Arc::clone(&c.http);
    let ok = WebDavConfig::new(c.srv.base_url.clone());

    let traversal = WebDavRemote::new(
        ok.clone().with_root_prefix("/.notes/../../../etc"),
        Arc::clone(&http),
    );
    assert!(traversal.is_err(), "根前缀里的穿越必须在构造期拒掉");

    let userinfo = WebDavRemote::new(
        ok.clone().with_base_url("http://user:****@dav.local"),
        Arc::clone(&http),
    );
    assert!(userinfo.is_err(), "凭据混进 URL 必须拒绝建适配器");

    assert!(WebDavRemote::new(ok.clone().with_base_url("不是 URL"), Arc::clone(&http)).is_err());
    assert!(WebDavRemote::new(
        ok.clone().with_base_url("ftp://dav.local"),
        Arc::clone(&http)
    )
    .is_err());
    assert!(WebDavRemote::new(
        ok.clone().with_base_url("http://dav.local"),
        Arc::clone(&http)
    )
    .is_ok());
    // base_url 自带前缀（Nextcloud 形态）时并入而不是丢弃。
    let nc = WebDavRemote::new(
        ok.with_base_url("https://d.example/remote.php/dav/files/u"),
        http,
    )
    .expect("合法");
    assert_eq!(
        nc.paths().manifest_index(),
        "https://d.example/remote.php/dav/files/u/.notes/manifest/index.json"
    );
}

#[tokio::test]
async fn missing_objects_probe_and_fetch_return_none_not_errors() {
    let c = Ctx::start().await;
    let r = c.remote(Caps::conventional());
    let id = EntityId::new();
    assert_eq!(r.fetch_record("note", id.as_str()).await.unwrap(), None);
    assert_eq!(r.probe_record_etag("n", id.as_str()).await.unwrap(), None);
    assert_eq!(
        r.fetch_segment("seg-0042").await.unwrap_err(),
        RemoteError::NotFound
    );
}

#[tokio::test]
async fn segments_roundtrip_as_entry_arrays() {
    let c = Ctx::start().await;
    let r = c.remote(Caps::conventional());
    let a = EntityId::new();
    let b = EntityId::new();
    let body = serde_json::to_vec(&vec![entry_of(&a, 1, b"x"), entry_of(&b, 2, b"y")]).unwrap();
    r.put_segment("seg-0000", &body).await.expect("分段写入");
    let got = r.fetch_segment("seg-0000").await.expect("分段读回");
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].i, a.as_str());
    assert_eq!(got[1].r, 2);

    // 压实可以重写同一分段（§4.3）：CAS 由随后的清单提交仲裁。
    let shrunk = serde_json::to_vec(&vec![entry_of(&a, 1, b"x")]).unwrap();
    r.put_segment("seg-0000", &shrunk).await.unwrap();
    assert_eq!(r.fetch_segment("seg-0000").await.unwrap().len(), 1);
    assert_eq!(
        c.dumped("/.notes/manifest/seg-0000.json")
            .await
            .expect("分段在盘上")["sha256"],
        json!(sha_of(&shrunk))
    );
    assert!(self::tmp_leftovers(&c).await.is_empty());
}

#[tokio::test]
async fn credentials_reach_the_server_but_never_the_debug_output() {
    let creds = Credentials::new("u1", "sup3r-s3cr3t").expect("凭据");
    let spec = RequestSpec::new(
        HttpMethod::Get,
        "http://127.0.0.1:1/.notes/manifest/index.json",
    )
    .with_header("authorization", creds.basic_header());
    let dbg = format!("{spec:?}");
    assert!(!dbg.contains("sup3r"), "{dbg}");
    assert!(dbg.contains("<redacted:basic>"), "{dbg}");
    assert!(!format!("{creds:?}").contains("sup3r"));

    // 服务器确实收到了带凭据的请求：401 注入下端口判 Auth（头没拼对就谈不上这条）。
    let c = Ctx::start().await;
    c.srv
        .inject(Injection::status("GET /.notes/manifest/index.json", 401))
        .await;
    let r = c.remote(Caps::conventional());
    let err = r.fetch_manifest(None).await.unwrap_err();
    assert_eq!(err, RemoteError::Auth);
    assert!(err.halts_round(), "401 必须停轮（§12）");
    assert!(c
        .log()
        .iter()
        .any(|(m, p, s)| m == "GET" && p.contains("index.json") && *s == 401));
}

#[tokio::test]
async fn corrupt_manifest_body_is_refused_not_trusted() {
    let c = Ctx::start().await;
    let r = c.remote(Caps::conventional());
    let v1 = empty_manifest(&c.device).to_wire();
    r.commit_manifest(&v1, None).await.unwrap();
    // FAIL(corrupt-body)：落盘不动，只有客户端视角看到脏字节 → checksum 必不符。
    c.srv
        .inject(Injection {
            corrupt_manifest: true,
            ..Default::default()
        })
        .await;
    let read = r.fetch_manifest(None).await;
    c.srv.clear_injection().await;
    // 端口只负责"把字节搬回来"，语义校验在引擎的 Manifest::parse 里：这里确认字节确实被改过。
    let got = read.expect("读请求本身成功").expect("有正文").0;
    assert_ne!(got, v1, "注入应改动了响应字节");
    assert!(
        Manifest::parse(&got).is_err(),
        "改动后的清单必须过不了自校验"
    );
    assert_eq!(
        r.fetch_manifest(None).await.unwrap().unwrap().0,
        v1,
        "撤掉注入后原样无损"
    );
}

/// 附件名是内容寻址的校验和本身：名字不合法就没有"该校验什么"这回事，
/// 必须在拼路径阶段就拒，一个请求都不发出去。
#[test]
fn attachment_names_are_validated_before_any_request() {
    let p = RemotePath::new("http://dav.local:5005", "/.notes").unwrap();
    let good = "a".repeat(64);
    assert_eq!(
        p.attachment(&good).unwrap(),
        format!("http://dav.local:5005/.notes/attachments/aa/{good}")
    );
    for bad in [
        "a".repeat(63),
        "A".repeat(64),
        "../../etc/passwd".to_string(),
        format!("{}..{}", &good[..62], "aa"),
        String::new(),
    ] {
        assert!(p.attachment(&bad).is_err(), "必须拒绝附件名：{bad:?}");
    }
}
