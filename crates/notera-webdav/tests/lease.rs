//! §11.4 尽力而为租约的适配器侧证据。
//!
//! 重点不是"能读写一个文件"，而是那三条会让同步停摆或让数据被覆盖的边界：
//! 过期即失效、自己的旧租约不算别人占着、列目录能力缺失时仍能靠已知设备让路，
//! 以及"租约这一层坏了绝不变成永不同步"。
use std::sync::Arc;
use std::time::Duration;

use notera_core::{Clock, DeviceId, SystemClock, Timestamp};
use notera_net::{HttpClient, ProxyProfile, Timeouts, TlsPolicy};
use notera_test_webdav::{Backend, Injection, Started, TestServer};
use notera_webdav::{Credentials, WebDavConfig, WebDavRemote};

const TTL: i64 = 60_000;

/// 相对现在的一个过期时刻（负数 = 已经过期）。
fn expiry(delta_ms: i64) -> String {
    Timestamp::from_millis(SystemClock.now().as_millis().unwrap_or_default() + delta_ms).to_string()
}

struct Ctx {
    srv: Started,
    http: Arc<HttpClient>,
}

async fn ctx() -> Ctx {
    let srv = TestServer::start(Backend::Mem).await;
    let http = Arc::new(
        HttpClient::build(&ProxyProfile::direct(), &TlsPolicy::Strict, Timeouts::short(Duration::from_secs(10)))
            .expect("出口客户端"),
    );
    Ctx { srv, http }
}

impl Ctx {
    fn remote(&self, device: &DeviceId) -> WebDavRemote {
        let cfg = WebDavConfig::new(self.srv.base_url.clone())
            .with_credentials(Credentials::new("notera-test", "sup3r-s3cr3t").expect("凭据"))
            .with_device(device.clone());
        WebDavRemote::new(cfg, Arc::clone(&self.http)).expect("合法配置")
    }
}

fn now_ms() -> i64 {
    SystemClock.now().as_millis().expect("时钟")
}

#[tokio::test]
async fn a_published_lease_is_visible_to_another_device_and_releasable() {
    let c = ctx().await;
    let a = DeviceId::new();
    let b = DeviceId::new();
    let ra = c.remote(&a);
    let rb = c.remote(&b);

    assert!(rb.lease_peers(&b.to_string(), &[]).await.unwrap().is_empty(), "开局不该有别人");
    ra.lease_publish(&a.to_string(), "tok-a", &expiry(TTL), 7).await.expect("贴租约");

    let peers = rb.lease_peers(&b.to_string(), &[]).await.unwrap();
    assert_eq!(peers.len(), 1, "另一台设备的租约必须看得见：{peers:?}");
    assert_eq!(peers[0].device, a.to_string());
    assert_eq!(peers[0].seq, 7, "seq 要随租约带过去（诊断用）");
    assert!(peers[0].is_fresh(now_ms()), "60s TTL 之内必须是新鲜的");

    ra.lease_release(&a.to_string()).await;
    assert!(rb.lease_peers(&b.to_string(), &[]).await.unwrap().is_empty(), "release 之后不该还挡路");
}

#[tokio::test]
async fn my_own_lease_never_blocks_me_and_is_overwritten() {
    let c = ctx().await;
    let me = DeviceId::new();
    let r = c.remote(&me);
    r.lease_publish(&me.to_string(), "tok-old", &expiry(TTL), 1).await.unwrap();
    // 同一台设备第二次启动：peers 里不该出现自己，且发布要能覆盖
    assert!(r.lease_peers(&me.to_string(), &[]).await.unwrap().is_empty(), "自己的租约被当成别人占着");
    r.lease_publish(&me.to_string(), "tok-new", &expiry(TTL), 2).await.unwrap();
    let raw = r.lease_peers(&DeviceId::new().to_string(), &[]).await.unwrap();
    assert_eq!(raw.len(), 1, "覆盖而不是叠加一份新文件：{raw:?}");
    assert_eq!(raw[0].token, "tok-new", "续期没覆盖掉旧内容");
}

#[tokio::test]
async fn an_expired_lease_is_no_longer_a_reason_to_yield() {
    let c = ctx().await;
    let a = DeviceId::new();
    let b = DeviceId::new();
    // 负 TTL = 贴一份早就过期的：设备崩溃留下的租约最坏挡住别人到 TTL 为止，不能永久挡
    c.remote(&a).lease_publish(&a.to_string(), "tok-dead", &expiry(-10_000), 3).await.expect("贴一份过期的");

    let peers = c.remote(&b).lease_peers(&b.to_string(), &[]).await.unwrap();
    assert_eq!(peers.len(), 1, "文件要能读出来（判断留给上层）：{peers:?}");
    assert!(!peers[0].is_fresh(now_ms()), "过期租约还被当成有效，就会永久挡住同步");
}

#[tokio::test]
async fn a_broken_propfind_still_yields_to_the_known_peer() {
    let c = ctx().await;
    let a = DeviceId::new();
    let b = DeviceId::new();
    c.remote(&a).lease_publish(&a.to_string(), "tok-a", &expiry(TTL), 1).await.unwrap();
    // 列目录坏掉的服务器（现实里存在）：靠"从清单学到的对手"这一条备用来源仍能看见
    c.srv.inject(Injection::status("PROPFIND *", 400)).await;
    let rb = c.remote(&b);
    assert!(rb.lease_peers(&b.to_string(), &[]).await.unwrap().is_empty(), "没有已知设备时确实看不见");
    let with_hint = rb.lease_peers(&b.to_string(), &[a.to_string()]).await.unwrap();
    assert_eq!(with_hint.len(), 1, "PROPFIND 坏了也要能给已知的那台让路：{with_hint:?}");
    assert_eq!(with_hint[0].device, a.to_string());
    c.srv.clear_injection().await;
}

#[tokio::test]
async fn a_lease_write_never_touches_the_data_areas() {
    let c = ctx().await;
    let a = DeviceId::new();
    c.remote(&a).lease_publish(&a.to_string(), "tok", &expiry(TTL), 1).await.unwrap();
    let dump = c.srv.fs_dump();
    // 写 locks/<dev>.json 会让服务器自动带上 locks 这一层目录，那是预期的；
    // 不能容忍的是碰到 records / manifest / attachments —— 那是要同步给所有设备的数据区。
    let touched = dump["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .filter_map(|e| e["path"].as_str())
        .filter(|p| p.contains("/records/") || p.contains("/manifest/") || p.contains("/attachments/"))
        .collect::<Vec<_>>();
    assert!(touched.is_empty(), "租约动了真实数据区就是污染清单：{touched:?}");
}

#[tokio::test]
async fn an_unreachable_server_is_an_error_for_publish_but_never_blocks_peers_read() {
    let c = ctx().await;
    let a = DeviceId::new();
    let r = c.remote(&a);
    c.srv.stop().await;
    // 贴不上：报错，由调用方记一条 debug —— 但不该有人把它当成"别人占着"
    assert!(r.lease_publish(&a.to_string(), "t", &expiry(TTL), 1).await.is_err());
    // 读不到别人：空表 + Ok，同步必须继续（§11.4：这一层坏了不能变成永不同步）
    assert!(r.lease_peers(&a.to_string(), &["其他".to_string()]).await.unwrap().is_empty());
    c.srv.restart().await;
}
