//! 一台**真能完成 TLS 握手**的源站（测试基建，仅 dev 链路）。
//!
//! 为什么要有它：`docs/PROXY.md` §6 写了四档 `TlsPolicy`，§8 写了"TLS 校验失败 ⇒ `Tls` 分类、
//! **不降级重试** ⇒ 用户看到 `! 证书不受信任`"。而在此之前，全仓每一条门禁都是
//! `TlsPolicy::Strict` 打在**明文 loopback** 上 —— 握手压根没发生，四档里没有任何一档
//! 被真证书验过。"证书不受信任"这句话要是坏了（例如哪天误开了 accept-invalid-certs），
//! 表现是**中间人可以改用户的笔记而客户端照单全收** —— 那是数据安全层的事，不是观感问题。
//!
//! 证书是**每次运行现造**的（rcgen，内存里），盘上不落任何密钥。链的形状有两种，因为
//! 这两件事在真实部署里不一样、失败原因也可能不一样：
//!
//! * [`TlsOrigin::start`] —— **自签根**当 CA（一张证书既是根又是签发者）；
//! * [`TlsOrigin::start_two_level`] —— **两级私有 CA**（根 CA → 中间 CA → 叶），服务器出示
//!   leaf + 中间 CA，客户端该拿到的是**根**的 PEM。内网/警务网那种"自建一个 CA、
//!   再发一张服务器证书"就是这一形。
//!
//! SAN 必带 **IP** `127.0.0.1`（客户端连的是 IP，rustls 拿 DNS 名匹配不算数），CA 必带
//! `keyCertSign`（rcgen 默认可用密钥用途是叶子那一套；少了它 Windows 校验器报的是
//! "无法验证证书的签名"而不是"issuer 不认" —— 第一版就撞在这里，差点把工装问题记成产品缺陷）。
//!
//! 这台源站**故意只做最小 HTTP/1.1**：握手完成后读掉请求、回一个固定 200，并分别数
//! `accepted`（TCP 连上来过几条，**含**握手就死的）与 `handled`（握手成、且真读到一个请求
//! 并回应了的）。这两个数才是这条门禁的牙齿 —— "TLS 失败时不许悄悄退化成明文/退避重试"
//! 的可证形态是 `accepted ≥ 1` 而 `handled == 0`（连上了、握手没成、什么数据都没交换），
//! 不是客户端自述"我拒绝了"。WebDAV 语义不在这里测（明文那台 `TestServer` 管）。
//!
//! crypto provider 是**显式指定**的（`builder_with_provider` + aws-lc-rs）：树里 rustls 的
//! 可用 provider 不止一个，用 `builder()` 那条"取进程默认"的路会在运行时以
//! "no process-level CryptoProvider available" 的形式炸 —— 那是工装坏，不是产品坏。
//!
//! **工装自检放在消费者那边**（`notera-net/tests/tls_policies.rs` 里那条"两种链形状都要能
//! 在跳过校验时完成握手"），不写在本 crate：本 crate 是 notera-net 的 dev-dependency，
//! 反向依赖会成环。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;

/// 一次生成出来的证书材料（只在内存里）。
struct Certs {
    /// 客户端该被交给的那份信任根 PEM（自签那形 = 那张 CA；两级那形 = **根** CA）。
    root_pem: String,
    /// 叶证书 PEM：不用于配策略，失败时人可拿它去 `openssl s_client` 复现。
    leaf_pem: String,
    /// 服务器出示的链（leaf 在前）。
    chain: Vec<CertificateDer<'static>>,
    /// 叶证书 DER 的 sha256（64 hex），给 `TlsPolicy::Pin`。
    leaf_sha256: String,
    /// 叶证书的私钥（PKCS#8 DER）。
    leaf_key: Vec<u8>,
}

/// CA 该有而 rcgen 默认**不给**的那几样用途。
fn ca_usages() -> Vec<rcgen::KeyUsagePurpose> {
    vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::CrlSign,
        rcgen::KeyUsagePurpose::DigitalSignature,
    ]
}

/// 测试形状固定用 RSA：ECDSA/RSA 的差别已经实测过（两种在 Windows 校验器下同样失败），
/// 才有资格把 G35 定性成"与算法无关"。
fn ca_pair() -> Result<rcgen::KeyPair, String> {
    rcgen::KeyPair::generate_for(&rcgen::PKCS_RSA_SHA256).map_err(|e| e.to_string())
}

fn ca_params(cn: &str) -> Result<rcgen::CertificateParams, String> {
    let mut p = rcgen::CertificateParams::new(vec![cn.to_string()]).map_err(|e| e.to_string())?;
    p.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    p.key_usages = ca_usages();
    Ok(p)
}

fn leaf_params() -> Result<rcgen::CertificateParams, String> {
    let mut p =
        rcgen::CertificateParams::new(vec!["127.0.0.1".to_string(), "localhost".to_string()])
            .map_err(|e| e.to_string())?;
    p.is_ca = rcgen::IsCa::NoCa;
    Ok(p)
}

/// 自签根那一形：CA 自己签自己。
fn certs_self_signed() -> Result<Certs, String> {
    let ca_key = ca_pair()?;
    let ca = ca_params("notera-test-ca")?
        .self_signed(&ca_key)
        .map_err(|e| e.to_string())?;
    let leaf_key = ca_pair()?;
    let leaf = leaf_params()?
        .signed_by(&leaf_key, &ca, &ca_key)
        .map_err(|e| e.to_string())?;
    Ok(Certs {
        root_pem: ca.pem(),
        leaf_pem: leaf.pem(),
        chain: vec![leaf.der().clone(), ca.der().clone()],
        leaf_sha256: hex_sha256(leaf.der().as_ref()),
        leaf_key: leaf_key.serialize_der(),
    })
}

/// 两级私有 CA 那一形：根 CA → 中间 CA → 叶。客户端拿到**根**的 PEM，
/// 链里给 leaf + 中间 CA（根不放进链 —— 那是信任锚该在的地方）。
fn certs_two_level() -> Result<Certs, String> {
    let root_key = ca_pair()?;
    let root = ca_params("notera-test-root")?
        .self_signed(&root_key)
        .map_err(|e| e.to_string())?;
    let inter_key = ca_pair()?;
    let inter = ca_params("notera-test-intermediate")?
        .signed_by(&inter_key, &root, &root_key)
        .map_err(|e| e.to_string())?;
    let leaf_key = ca_pair()?;
    let leaf = leaf_params()?
        .signed_by(&leaf_key, &inter, &inter_key)
        .map_err(|e| e.to_string())?;
    Ok(Certs {
        root_pem: root.pem(),
        leaf_pem: leaf.pem(),
        chain: vec![leaf.der().clone(), inter.der().clone()],
        leaf_sha256: hex_sha256(leaf.der().as_ref()),
        leaf_key: leaf_key.serialize_der(),
    })
}

/// 现造的证书 + 一台听在临时端口上的 TLS 源站。
pub struct TlsOrigin {
    addr: SocketAddr,
    root_pem: String,
    leaf_pem: String,
    leaf_sha256: String,
    accepted: Arc<AtomicU64>,
    handled: Arc<AtomicU64>,
}

impl TlsOrigin {
    /// **自签根**那一形：`ca_pem()` 就是那张自签 CA 自己。
    pub async fn start() -> Result<TlsOrigin, String> {
        Self::serve(certs_self_signed()?).await
    }

    /// **两级私有 CA**那一形：`ca_pem()` 是根 CA，链里是叶 + 中间 CA。
    pub async fn start_two_level() -> Result<TlsOrigin, String> {
        Self::serve(certs_two_level()?).await
    }

    async fn serve(certs: Certs) -> Result<TlsOrigin, String> {
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(certs.leaf_key));
        let provider = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider();
        let mut cfg = ServerConfig::builder_with_provider(Arc::new(provider))
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_no_client_auth()
            .with_single_cert(certs.chain, key)
            .map_err(|e| e.to_string())?;
        // 钉死 http/1.1：reqwest 开了 `http2` feature，让 ALPN 自己去选会把它带进 h2 前置序列，
        // 而这里要测的是**证书**，不是帧格式。
        cfg.alpn_protocols = vec![b"http/1.1".to_vec()];

        let acceptor = Arc::new(TlsAcceptor::from(Arc::new(cfg)));
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|e| format!("绑定 TLS 源站失败：{e}"))?;
        let addr = listener.local_addr().map_err(|e| e.to_string())?;

        let accepted = Arc::new(AtomicU64::new(0));
        let handled = Arc::new(AtomicU64::new(0));
        {
            let (acc, a, h) = (acceptor, accepted.clone(), handled.clone());
            tokio::spawn(async move {
                loop {
                    let Ok((sock, _)) = listener.accept().await else {
                        return;
                    };
                    a.fetch_add(1, Ordering::SeqCst);
                    let acc = acc.clone();
                    let h = h.clone();
                    tokio::spawn(async move {
                        // 握手不成 ⇒ 这条连接到此为止，**不**计入 handled。
                        let Ok(io) = acc.accept(sock).await else {
                            return;
                        };
                        let mut rd = BufReader::new(io);
                        let mut line = String::new();
                        if rd.read_line(&mut line).await.unwrap_or(0) == 0 {
                            return;
                        }
                        // 吃掉头部到空行（测试只发 GET，不带 body）。
                        loop {
                            let mut hdr = String::new();
                            match rd.read_line(&mut hdr).await {
                                Ok(0) => break,
                                Ok(_) if hdr.trim().is_empty() => break,
                                Ok(_) => {}
                                Err(_) => break,
                            }
                        }
                        h.fetch_add(1, Ordering::SeqCst);
                        let body: &[u8] = br#"{"ok":true}"#;
                        let head = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                            body.len()
                        );
                        let io = rd.get_mut();
                        let _ = io.write_all(head.as_bytes()).await;
                        let _ = io.write_all(body).await;
                        let _ = io.flush().await;
                    });
                }
            });
        }

        Ok(TlsOrigin {
            addr,
            root_pem: certs.root_pem,
            leaf_pem: certs.leaf_pem,
            leaf_sha256: certs.leaf_sha256,
            accepted,
            handled,
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `https://127.0.0.1:<port>/.notes` —— 客户端只认这个 origin。
    pub fn base_url(&self) -> String {
        format!("https://{}/.notes", self.addr)
    }

    /// 信任根的 PEM：喂给 `TlsPolicy::CaBundle`，这就是 §6 说的"内网自签主路径"。
    /// 自签那一形里它就是那张 CA 自己。
    pub fn ca_pem(&self) -> &str {
        &self.root_pem
    }

    /// 叶证书 PEM（不用于配策略；失败时人可拿它去 `openssl s_client` 复现）。
    pub fn leaf_pem(&self) -> &str {
        &self.leaf_pem
    }

    /// 叶证书 DER 的 sha256（64 hex）：喂给 `TlsPolicy::Pin`。
    pub fn leaf_sha256(&self) -> &str {
        &self.leaf_sha256
    }

    /// TCP 连上来过几条（**含**握手就失败的）—— 这一样给出"客户端到底试了几回"。
    pub fn accepted(&self) -> u64 {
        self.accepted.load(Ordering::SeqCst)
    }

    /// 握手成、且真读到一个请求并回应了的条数 —— 换过数据才算。
    pub fn handled(&self) -> u64 {
        self.handled.load(Ordering::SeqCst)
    }
}

fn hex_sha256(b: &[u8]) -> String {
    let d = Sha256::digest(b);
    let mut s = String::with_capacity(64);
    for x in d {
        s.push_str(&format!("{x:02x}"));
    }
    s
}
