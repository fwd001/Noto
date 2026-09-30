//! 一台**真能完成 TLS 握手**的源站（测试基建，仅 dev 链路）。
//!
//! 为什么要有它：`docs/PROXY.md` §6 写了四档 `TlsPolicy`，§8 写了"TLS 校验失败 ⇒ `Tls` 分类、
//! **不降级重试** ⇒ 用户看到 `! 证书不受信任`"。而在此之前，全仓每一条门禁都是
//! `TlsPolicy::Strict` 打在**明文 loopback** 上 —— 握手压根没发生，四档里没有任何一档
//! 被真证书验过。"证书不受信任"这句话要是坏了（例如哪天误开了 accept-invalid-certs），
//! 表现是**中间人可以改用户的笔记而客户端照单全收** —— 那是数据安全层的事，不是观感问题。
//!
//! 证书是**每次运行现造**的（rcgen，内存里）：一张自签 CA + 一张签给它下面的叶证书，
//! SAN 带 **IP** `127.0.0.1`（客户端连的是 IP，rustls 拿 DNS 名匹配不算数）；盘上不落任何密钥。
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

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;

/// 现造的一对证书 + 一台听在临时端口上的 TLS 源站。
pub struct TlsOrigin {
    addr: SocketAddr,
    ca_pem: String,
    leaf_pem: String,
    leaf_sha256: String,
    accepted: Arc<AtomicU64>,
    handled: Arc<AtomicU64>,
}

impl TlsOrigin {
    /// 起一台。**CA PEM 与叶指纹出自同一次生成**：`CaBundle` 与 `Pin` 两档要能互相指认，
    /// 分开造就变成在测两个东西。
    pub async fn start() -> Result<TlsOrigin, String> {
        let err = |e: rcgen::Error| e.to_string();
        // 密钥算法：见下面 CA 那段注释里的实测——Windows 的平台校验器对这条测试链报
        // `NTE_BAD_SIGNATURE`，先用 RSA 试出它到底是"不认 ECDSA 测试链"还是"不认任何用户加的根"。
        let alg = &rcgen::PKCS_RSA_SHA256;

        // ① 自签 CA。
        //
        // `key_cert_sign` 必须显式给：rcgen 的默认可用密钥用途是**叶子**那一套
        // （digital_signature / key_encipherment），少了 keyCertSign，Windows 的平台校验器
        // 会拒用这张 CA 去验签，报出来的不是" issuer 不认"而是 `无法验证证书的签名
        // (os error -2146869244)` —— 第一版就是撞在这里，看起来像 `CaBundle` 没接线，
        // 其实是我造的 CA 不合法。
        let mut ca_params =
            rcgen::CertificateParams::new(vec!["notera-test-ca".to_string()]).map_err(err)?;
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![
            rcgen::KeyUsagePurpose::KeyCertSign,
            rcgen::KeyUsagePurpose::CrlSign,
            rcgen::KeyUsagePurpose::DigitalSignature,
        ];
        let ca_key = rcgen::KeyPair::generate_for(alg).map_err(err)?;
        let ca = ca_params.self_signed(&ca_key).map_err(err)?;

        // ② CA 签出的叶证书，SAN 必须带 IP `127.0.0.1`。
        let mut leaf_params =
            rcgen::CertificateParams::new(vec!["127.0.0.1".to_string(), "localhost".to_string()])
                .map_err(err)?;
        leaf_params.is_ca = rcgen::IsCa::NoCa;
        let leaf_key = rcgen::KeyPair::generate_for(alg).map_err(err)?;
        let leaf = leaf_params
            .signed_by(&leaf_key, &ca, &ca_key)
            .map_err(err)?;

        let leaf_sha256 = hex_sha256(leaf.der().as_ref());

        let certs: Vec<CertificateDer<'static>> = vec![leaf.der().clone(), ca.der().clone()];
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der()));
        let provider = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider();
        let mut cfg = ServerConfig::builder_with_provider(Arc::new(provider))
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_no_client_auth()
            .with_single_cert(certs, key)
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
            ca_pem: ca.pem(),
            leaf_pem: leaf.pem(),
            leaf_sha256,
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

    /// 协议方案（`https`），给要自己拼 URL 的测试用。
    pub fn scheme(&self) -> &'static str {
        "https"
    }

    /// 自签 CA 的 PEM：喂给 `TlsPolicy::CaBundle`，这就是 §6 说的"内网自签主路径"。
    pub fn ca_pem(&self) -> &str {
        &self.ca_pem
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
