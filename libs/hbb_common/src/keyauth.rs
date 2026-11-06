//! 密钥鉴权：Master 用全局私钥证明身份，Service 用绑定的公钥验签；Master 另用「钉住的主机公钥」
//! 确认对面是地址簿里那台机器。
//!
//! 两个方向的身份都由本模块自己的握手消息承担，`crate::tcp` 的 Kx 只负责建立加密通道
//! （它是一次**无签名**的临时密钥交换，本身不证明任何一端的身份）：
//!
//! * Master → Service：`AuthProof.signature`，用 Master 的全局私钥签 `AUTH_DOMAIN ‖ transcript ‖ nonce`。
//! * Service → Master：`AuthNonce.host_pk` + `AuthNonce.host_proof`（`sign_host` / `verify_host`），
//!   用 Service 自己的主机密钥签同一套输入；Master 验完再把公钥交给 `PinnedHosts` 做 TOFU。
//!
//! 设计与取舍见 `architecture.md` 里的“身份”一节。

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context as _, Result};
use protobuf::Message;
use serde_derive::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sodiumoxide::crypto::sign;


/// 认证协议版本，与 `AuthNonce.version` 对应；两端不一致时拒绝，不降级。
pub const AUTH_VERSION: u32 = 1;

/// 签名输入的域前缀。protobuf 消息不自带类型，同一把密钥签的不同消息必须靠前缀区分，
/// 否则一张签名可以被拿去冒充另一类消息。
pub const AUTH_DOMAIN: &[u8] = b"landesk-auth-v1";

/// 挑战长度。
pub const NONCE_LEN: usize = 32;

/// 主机证明的域前缀，与 Master 证明分开：两把钥匙、两种语义，不能互相冒充。
pub const HOST_DOMAIN: &[u8] = b"landesk-host-v1";

/// Master 持有的全局密钥对。所有 Master 共用同一把私钥，所有 Service 绑同一把公钥。
pub struct GlobalKeyPair {
    pub pk: sign::PublicKey,
    pub sk: sign::SecretKey,
}

impl GlobalKeyPair {
    pub fn generate() -> Self {
        let (pk, sk) = sign::gen_keypair();
        Self { pk, sk }
    }

    pub fn from_secret_bytes(bytes: &[u8]) -> Result<Self> {
        let sk = sign::SecretKey::from_slice(bytes).context("invalid secret key length")?;
        let pk = sk.public_key();
        Ok(Self { pk, sk })
    }

    /// 私钥原始字节。调用方负责保管：不写日志、不进 CSV、随地址簿导出的只有公钥。
    pub fn secret_bytes(&self) -> &[u8] {
        &self.sk.0
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading private key {}", path.display()))?;
        let bytes = from_hex(text.trim()).context("private key file is not valid hex")?;
        let key = Self::from_secret_bytes(&bytes)?;
        // 升级路径：早先的版本用 `fs::write` 写过 0644 的私钥，读的时候顺手收紧一次，
        // 而不是指望用户重新生成。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(err) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
                log::warn!("cannot tighten permissions of {}: {err}", path.display());
            }
        }
        Ok(key)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        // 私钥是全网通行证：非 Windows 上以 0600 创建，别让 umask 变出 0644。
        // Windows 没有这个模式位，保护靠文件系统 ACL（"只允许当前用户"）。
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(path)
                .with_context(|| format!("creating private key {}", path.display()))?;
            file.write_all(to_hex(self.secret_bytes()).as_bytes())
                .with_context(|| format!("writing private key {}", path.display()))?;
            // 已存在的文件不会被 `mode` 改权限，所以再收紧一次。
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        #[cfg(not(unix))]
        {
            std::fs::write(path, to_hex(self.secret_bytes()))
                .with_context(|| format!("writing private key {}", path.display()))?;
        }
        Ok(())
    }
}

/// 主机公钥的指纹：SHA-256 的前 16 字节，写成十六进制、每 2 字节一组、共 8 组。
///
/// 每组固定 4 位（`{:04X}`）：拼成 u16 后高位可能是 0，用 `{:02X}` 会输出 2–4 位不等，
/// 而操作者在 TOFU 时要逐个字符对读这个指纹。
pub fn fingerprint_bytes(pk: &[u8]) -> String {
    fingerprint_from_digest(&Sha256::digest(pk))
}

/// 把摘要格式化成指纹：前 16 字节、每 2 字节一组、每组固定 4 位。
///
/// 拆出来是为了能用构造好的摘要做测试——否则只能靠某个摘要字节恰好是 0x00 才能暴露`{:02X}` 吃前导零的问题。
fn fingerprint_from_digest(digest: &[u8]) -> String {
    let groups: Vec<String> = digest[..16]
        .chunks(2)
        .map(|pair| format!("{:04X}", (u16::from(pair[0]) << 8) | u16::from(pair[1])))
        .collect();
    groups.join("-")
}

pub fn fingerprint(pk: &sign::PublicKey) -> String {
    fingerprint_bytes(&pk.0)
}

/// 密钥交换 transcript 的摘要，作为签名输入的一部分：同一把私钥的签名因此只对这一次连接有效。
pub fn transcript_digest(
    initiator_pk: &[u8],
    responder_pk: &[u8],
    advertised: u32,
    picked: u32,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(initiator_pk);
    hasher.update(responder_pk);
    hasher.update(advertised.to_le_bytes());
    hasher.update(picked.to_le_bytes());
    hasher.finalize().into()
}

fn signed_payload(transcript: &[u8; 32], nonce: &[u8]) -> Vec<u8> {
    domain_payload(AUTH_DOMAIN, transcript, nonce)
}

fn domain_payload(domain: &[u8], transcript: &[u8; 32], nonce: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(domain.len() + transcript.len() + nonce.len());
    payload.extend_from_slice(domain);
    payload.extend_from_slice(transcript);
    payload.extend_from_slice(nonce);
    payload
}

/// 生成挑战。
pub fn new_nonce() -> [u8; NONCE_LEN] {
    let mut nonce = [0u8; NONCE_LEN];
    sodiumoxide::randombytes::randombytes_into(&mut nonce);
    nonce
}

/// Service 侧：用主机私钥对同一次挑战签名，证明"我是那把主机密钥的主人"。
pub fn sign_host(sk: &sign::SecretKey, transcript: &[u8; 32], nonce: &[u8]) -> Vec<u8> {
    sign::sign(&domain_payload(HOST_DOMAIN, transcript, nonce), sk)
}

/// Master 侧：验证主机证明。主机公钥由 Service 在挑战里给出，是否可信由指纹库（TOFU）回答。
pub fn verify_host(
    host_pk: &sign::PublicKey,
    transcript: &[u8; 32],
    nonce: &[u8],
    proof: &[u8],
) -> std::result::Result<(), RejectReason> {
    if nonce.len() != NONCE_LEN {
        return Err(RejectReason::BadHostProof);
    }
    match sign::verify(proof, host_pk) {
        Ok(message) if message == domain_payload(HOST_DOMAIN, transcript, nonce) => Ok(()),
        _ => Err(RejectReason::BadHostProof),
    }
}

/// Master 侧：对 transcript 摘要与挑战签名。
pub fn sign_auth(kp: &GlobalKeyPair, transcript: &[u8; 32], nonce: &[u8]) -> Vec<u8> {
    sign::sign(&signed_payload(transcript, nonce), &kp.sk)
}

/// Service 侧：用绑定的公钥验签。`None` 表示这台 Service 还没绑定公钥。
/// 服务端验签：只要签名命中**集合里任意一把**就通过。
///
/// 一个网络里可能有多台 Master，每台一把密钥；空集合视为未绑定（一律拒）。
pub fn verify_auth_any(
    bound_pks: &[sign::PublicKey],
    transcript: &[u8; 32],
    nonce: &[u8],
    signature: &[u8],
) -> std::result::Result<(), RejectReason> {
    if bound_pks.is_empty() {
        return Err(RejectReason::NoBoundKey);
    }
    if nonce.len() != NONCE_LEN {
        return Err(RejectReason::BadSignature);
    }
    let expected = signed_payload(transcript, nonce);
    for pk in bound_pks {
        if let Ok(message) = sign::verify(signature, pk) {
            if message == expected {
                return Ok(());
            }
        }
    }
    Err(RejectReason::BadSignature)
}

pub fn verify_auth(
    bound_pk: Option<&sign::PublicKey>,
    transcript: &[u8; 32],
    nonce: &[u8],
    signature: &[u8],
) -> std::result::Result<(), RejectReason> {
    match bound_pk {
        Some(pk) => verify_auth_any(std::slice::from_ref(pk), transcript, nonce, signature),
        None => Err(RejectReason::NoBoundKey),
    }
}

/// 这条连接在 Service 侧的角色：持令牌的控制者，或只看画面的观察者。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthRole {
    Controller,
    Viewer,
}

/// 一次连接被拒的原因，日志一行的内容就是它。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// 这台 Service 还没绑定公钥。
    NoBoundKey,
    /// 签名不成立：私钥不对、挑战被改、或 transcript 不匹配。
    BadSignature,
    /// 对面给的主机公钥与地址簿里钉住的不一致。
    HostFingerprintMismatch,
    /// 主机证明不成立：主机私钥不对，或被改过挑战。
    BadHostProof,
    /// 认证协议版本不一致。
    VersionMismatch,
    /// 对方拿来的身份不在这台 Service 绑定的集合里。
    KeyNotAllowed,
    /// 证明到达之前就收到了会话消息。
    NoProofYet,
}

impl RejectReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NoBoundKey => "no-bound-key",
            Self::BadSignature => "bad-signature",
            Self::HostFingerprintMismatch => "host-fingerprint-mismatch",
            Self::BadHostProof => "bad-host-proof",
            Self::VersionMismatch => "version-mismatch",
            Self::KeyNotAllowed => "key-not-allowed",
            Self::NoProofYet => "no-proof-yet",
        }
    }
}

impl std::fmt::Display for RejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 拒绝时服务端留下的那一行日志。
pub fn reject_log_line(reason: RejectReason, addr: &str) -> String {
    format!("landesk: reject {reason} from {addr}")
}

/// 更直白的拒绕行：带上“对方声明的身份”与“本机绑定的身份”，方便对号入座。
pub fn reject_log_line_detail(
    reason: RejectReason,
    addr: &str,
    presented: Option<&str>,
    bound_pks: &[sign::PublicKey],
) -> String {
    let presented = presented
        .map(|fp| format!("presented key {fp}"))
        .unwrap_or_else(|| "presented key unknown (older client)".to_owned());
    let bound = if bound_pks.is_empty() {
        "no key bound".to_owned()
    } else {
        let list = bound_pks
            .iter()
            .take(8)
            .map(fingerprint)
            .collect::<Vec<_>>()
            .join(", ");
        format!("bound {}: {list}", bound_pks.len())
    };
    format!("landesk: reject {reason} from {addr} — {presented}; {bound}")
}

/// Master 侧对目标主机的判断结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinOutcome {
    /// 该地址首次连接，公钥已记录，标记为未验证。
    New,
    /// 与已记录的公钥一致。
    Verified,
    /// 与已记录的公钥不一致：拒绝连接，旧记录保持不动。
    Changed,
}

/// 地址簿里钉住的一台主机。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinnedHost {
    pub host_pk: String,
    pub fingerprint: String,
    pub verified: bool,
}

/// Master 侧的指纹库：`"addr:port" -> 钉住的主机公钥`。
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PinnedHosts {
    hosts: BTreeMap<String, PinnedHost>,
}

impl PinnedHosts {
    pub fn pin_or_check(&mut self, addr: &str, host_pk: &sign::PublicKey) -> PinOutcome {
        let pk = to_hex(&host_pk.0);
        match self.hosts.get(addr) {
            None => {
                self.hosts.insert(
                    addr.to_owned(),
                    PinnedHost {
                        host_pk: pk,
                        fingerprint: fingerprint(host_pk),
                        verified: false,
                    },
                );
                PinOutcome::New
            }
            Some(known) if known.host_pk == pk => {
                // 命中时顺手把显示串刷新成当前格式（指纹格式随版本可能变，例如修掉前导零）。
                let current = fingerprint(host_pk);
                if known.fingerprint != current {
                    if let Some(slot) = self.hosts.get_mut(addr) {
                        slot.fingerprint = current;
                    }
                }
                PinOutcome::Verified
            }
            Some(_) => PinOutcome::Changed,
        }
    }

    pub fn get(&self, addr: &str) -> Option<&PinnedHost> {
        self.hosts.get(addr)
    }

    /// 操作者确认过这台机器之后调用；地址簿里它就从「未验证」变成已知。
    pub fn mark_verified(&mut self, addr: &str) -> bool {
        match self.hosts.get_mut(addr) {
            Some(host) => {
                host.verified = true;
                true
            }
            None => false,
        }
    }

    /// 忘掉一台机器，下次连接重新走首次信任。
    pub fn forget(&mut self, addr: &str) -> bool {
        self.hosts.remove(addr).is_some()
    }

    pub fn len(&self) -> usize {
        self.hosts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }

    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("parsing pinned hosts at {}", path.display())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let text = serde_json::to_string_pretty(self).context("serializing pinned hosts")?;
        std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
    }
}

/// 认证步骤的超时；到点没等到对面的消息即按拒绝处理。
pub const AUTH_TIMEOUT_MS: u64 = 10_000;

/// Service 侧：在已建立（已加密）的通道上发挑战并验签。
///
/// 挑战里带上自己的主机公钥与主机签名，Master 据此判断对面是不是它认识的那台机器。
/// 未绑定公钥时不发任何东西，直接拒——少暴露一轮握手。返回的原因供调用方写成日志一行。
pub async fn challenge_and_verify(
    stream: &mut crate::tcp::FramedStream,
    bound_pk: Option<&sign::PublicKey>,
    host_pk: &sign::PublicKey,
    host_sk: &sign::SecretKey,
    transcript: &[u8; 32],
) -> std::result::Result<AuthRole, RejectReason> {
    match bound_pk {
        Some(pk) => {
            challenge_and_verify_any(
                stream,
                std::slice::from_ref(pk),
                host_pk,
                host_sk,
                transcript,
            )
            .await
        }
        None => Err(RejectReason::NoBoundKey),
    }
}

/// 认证失败时能拿到的全部信息：给日志用（哪把身份被拒、这台 Service 绑了几把）。
pub struct AuthFailure {
    pub reason: RejectReason,
    /// 对方声明的 client 公钥指纹（老客户端不带 `client_pk` 时为 None）。
    pub presented: Option<String>,
    /// 本机绑定的公钥数量。
    pub bound: usize,
}

/// 同一件事的集合版：命中任意一把绑定的 client 公钥即通过。
pub async fn challenge_and_verify_any(
    stream: &mut crate::tcp::FramedStream,
    bound_pks: &[sign::PublicKey],
    host_pk: &sign::PublicKey,
    host_sk: &sign::SecretKey,
    transcript: &[u8; 32],
) -> std::result::Result<AuthRole, RejectReason> {
    challenge_and_verify_any_with_info(stream, bound_pks, host_pk, host_sk, transcript)
        .await
        .map_err(|failure| failure.reason)
}

pub async fn challenge_and_verify_any_with_info(
    stream: &mut crate::tcp::FramedStream,
    bound_pks: &[sign::PublicKey],
    host_pk: &sign::PublicKey,
    host_sk: &sign::SecretKey,
    transcript: &[u8; 32],
) -> std::result::Result<AuthRole, AuthFailure> {
    let bound = bound_pks.len();
    let fail = |reason| AuthFailure {
        reason,
        presented: None,
        bound,
    };
    if bound_pks.is_empty() {
        return Err(fail(RejectReason::NoBoundKey));
    }
    let nonce = new_nonce();
    let mut challenge = crate::rendezvous_proto::RendezvousMessage::new();
    challenge.set_auth_nonce(crate::rendezvous_proto::AuthNonce {
        nonce: bytes::Bytes::copy_from_slice(&nonce),
        version: AUTH_VERSION,
        host_pk: bytes::Bytes::copy_from_slice(&host_pk.0),
        host_proof: sign_host(host_sk, transcript, &nonce).into(),
        ..Default::default()
    });
    if stream.send(&challenge).await.is_err() {
        return Err(fail(RejectReason::NoProofYet));
    }
    match stream.next_timeout(AUTH_TIMEOUT_MS).await {
        Some(Ok(bytes)) => match crate::rendezvous_proto::RendezvousMessage::parse_from_bytes(&bytes)
        {
            Ok(crate::rendezvous_proto::RendezvousMessage {
                union: Some(crate::rendezvous_proto::rendezvous_message::Union::AuthProof(proof)),
                ..
            }) => {
                // 对方自报的公钥（公开数据）：有它就能直接指名“拒绝的是哪把身份”，
                // 也能在最坏情况下只验一次而不是遍历整个集合。老客户端不带，则回落遍历。
                let presented = sign::PublicKey::from_slice(&proof.client_pk);
                let presented_fp = presented.as_ref().map(fingerprint);
                let candidates: Vec<sign::PublicKey> = match &presented {
                    Some(pk) => {
                        if !bound_pks.contains(pk) {
                            let failure = AuthFailure {
                                reason: RejectReason::KeyNotAllowed,
                                presented: presented_fp,
                                bound,
                            };
                            send_auth_err(stream, failure.reason).await;
                            return Err(failure);
                        }
                        vec![pk.clone()]
                    }
                    None => bound_pks.to_vec(),
                };
                match verify_auth_any(&candidates, transcript, &nonce, &proof.signature) {
                    Ok(()) => {
                        // 显式告诉对面“过了”：否则 Master 分不清“被拒”与“连接没了”。
                        let mut ok = crate::rendezvous_proto::RendezvousMessage::new();
                        ok.set_auth_ok(crate::rendezvous_proto::AuthOk::default());
                        if stream.send(&ok).await.is_err() {
                            return Err(AuthFailure {
                                reason: RejectReason::NoProofYet,
                                presented: presented_fp,
                                bound,
                            });
                        }
                        Ok(if proof.control {
                            AuthRole::Controller
                        } else {
                            AuthRole::Viewer
                        })
                    }
                    Err(reason) => {
                        let failure = AuthFailure {
                            reason,
                            presented: presented_fp,
                            bound,
                        };
                        send_auth_err(stream, failure.reason).await;
                        Err(failure)
                    }
                }
            }
            _ => Err(fail(RejectReason::NoProofYet)),
        },
        _ => Err(fail(RejectReason::NoProofYet)),
    }
}

/// Master 侧：等对面的挑战，先验证主机身份（并按指纹库做 TOFU），再用全局私钥回证明。
///
/// `control` 为真才申请控制令牌；为假时以观察者身份接入（只看、不抢操作权）。
/// 返回指纹库对这台机器的结论：`New` 表示首次连接、已记录并标为未验证。
pub async fn answer_challenge(
    stream: &mut crate::tcp::FramedStream,
    kp: &GlobalKeyPair,
    transcript: &[u8; 32],
    pinned: &mut PinnedHosts,
    addr: &str,
    control: bool,
) -> std::result::Result<PinOutcome, RejectReason> {
    let bytes = match stream.next_timeout(AUTH_TIMEOUT_MS).await {
        Some(Ok(bytes)) => bytes,
        _ => return Err(RejectReason::NoProofYet),
    };
    let message = crate::rendezvous_proto::RendezvousMessage::parse_from_bytes(&bytes)
        .map_err(|_| RejectReason::NoProofYet)?;
    let challenge = match message.union {
        Some(crate::rendezvous_proto::rendezvous_message::Union::AuthNonce(challenge)) => challenge,
        _ => return Err(RejectReason::NoProofYet),
    };
    if challenge.version != AUTH_VERSION {
        return Err(RejectReason::VersionMismatch);
    }
    let host_pk = sign::PublicKey::from_slice(&challenge.host_pk).ok_or(RejectReason::BadHostProof)?;
    verify_host(&host_pk, transcript, &challenge.nonce, &challenge.host_proof)?;
    let outcome = pinned.pin_or_check(addr, &host_pk);
    if outcome == PinOutcome::Changed {
        return Err(RejectReason::HostFingerprintMismatch);
    }
    let mut proof = crate::rendezvous_proto::RendezvousMessage::new();
    proof.set_auth_proof(crate::rendezvous_proto::AuthProof {
        signature: sign_auth(kp, transcript, &challenge.nonce).into(),
        control,
        client_pk: bytes::Bytes::copy_from_slice(&kp.pk.0),
        ..Default::default()
    });
    stream.send(&proof).await.map_err(|_| RejectReason::NoProofYet)?;
    // 等 Service 的明示：过了（AuthOk）或者被拒（AuthErr 带原因）。
    match stream.next_timeout(AUTH_TIMEOUT_MS).await {
        Some(Ok(bytes)) => match crate::rendezvous_proto::RendezvousMessage::parse_from_bytes(&bytes)
        {
            Ok(crate::rendezvous_proto::RendezvousMessage {
                union: Some(crate::rendezvous_proto::rendezvous_message::Union::AuthOk(_)),
                ..
            }) => Ok(outcome),
            Ok(crate::rendezvous_proto::RendezvousMessage {
                union: Some(crate::rendezvous_proto::rendezvous_message::Union::AuthErr(err)),
                ..
            }) => Err(reject_reason_from_str(&err.reason)),
            _ => Err(RejectReason::BadSignature),
        },
        _ => Err(RejectReason::BadSignature),
    }
}

/// 把拒绝原因明确告诉对端，然后由调用方断开。没有这一步，对端只能说“连接没了”。
async fn send_auth_err(stream: &mut crate::tcp::FramedStream, reason: RejectReason) {
    let mut msg = crate::rendezvous_proto::RendezvousMessage::new();
    msg.set_auth_err(crate::rendezvous_proto::AuthErr {
        reason: format!("{reason}"),
        ..Default::default()
    });
    let _ = stream.send(&msg).await;
}

/// 把对端回过来的拒绝原因字符串还原成枚举（未知值归为 `BadSignature`）。
pub fn reject_reason_from_str(text: &str) -> RejectReason {
    match text {
        "no-bound-key" => RejectReason::NoBoundKey,
        "key-not-allowed" => RejectReason::KeyNotAllowed,
        "host-fingerprint-mismatch" => RejectReason::HostFingerprintMismatch,
        "bad-host-proof" => RejectReason::BadHostProof,
        "version-mismatch" => RejectReason::VersionMismatch,
        "no-proof-yet" => RejectReason::NoProofYet,
        _ => RejectReason::BadSignature,
    }
}

fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn from_hex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> GlobalKeyPair {
        let _ = sodiumoxide::init();
        GlobalKeyPair::generate()
    }

    fn transcript() -> [u8; 32] {
        transcript_digest(&[1u8; 32], &[2u8; 32], 1, 1)
    }

    #[test]
    fn signature_roundtrip() {
        let kp = setup();
        let nonce = new_nonce();
        let digest = transcript();
        let signature = sign_auth(&kp, &digest, &nonce);
        assert_eq!(verify_auth(Some(&kp.pk), &digest, &nonce, &signature), Ok(()));
    }

    #[test]
    fn any_key_in_the_set_is_accepted() {
        let kp = setup();
        let other = setup();
        let nonce = new_nonce();
        let digest = transcript();
        let signature = sign_auth(&kp, &digest, &nonce);
        let set = vec![other.pk, kp.pk];
        assert_eq!(verify_auth_any(&set, &digest, &nonce, &signature), Ok(()));
    }

    #[test]
    fn a_signature_outside_the_set_is_refused() {
        let kp = setup();
        let stranger = setup();
        let nonce = new_nonce();
        let digest = transcript();
        let signature = sign_auth(&stranger, &digest, &nonce);
        assert_eq!(
            verify_auth_any(std::slice::from_ref(&kp.pk), &digest, &nonce, &signature),
            Err(RejectReason::BadSignature)
        );
    }

    #[test]
    fn an_empty_set_refuses_everything() {
        let kp = setup();
        let nonce = new_nonce();
        let digest = transcript();
        let signature = sign_auth(&kp, &digest, &nonce);
        assert_eq!(
            verify_auth_any(&[], &digest, &nonce, &signature),
            Err(RejectReason::NoBoundKey)
        );
    }

    #[test]
    fn tampered_nonce_is_refused() {
        let kp = setup();
        let nonce = new_nonce();
        let digest = transcript();
        let signature = sign_auth(&kp, &digest, &nonce);
        let mut other = nonce;
        other[0] ^= 0xff;
        assert_eq!(
            verify_auth(Some(&kp.pk), &digest, &other, &signature),
            Err(RejectReason::BadSignature)
        );
    }

    #[test]
    fn tampered_transcript_is_refused() {
        let kp = setup();
        let nonce = new_nonce();
        let signature = sign_auth(&kp, &transcript(), &nonce);
        let other = transcript_digest(&[1u8; 32], &[3u8; 32], 1, 1);
        assert_eq!(
            verify_auth(Some(&kp.pk), &other, &nonce, &signature),
            Err(RejectReason::BadSignature)
        );
    }

    #[test]
    fn wrong_key_is_refused() {
        let master = setup();
        let service = setup();
        let nonce = new_nonce();
        let digest = transcript();
        let signature = sign_auth(&master, &digest, &nonce);
        assert_eq!(
            verify_auth(Some(&service.pk), &digest, &nonce, &signature),
            Err(RejectReason::BadSignature)
        );
    }

    #[test]
    fn unbound_service_refuses_everything() {
        let kp = setup();
        let nonce = new_nonce();
        let digest = transcript();
        let signature = sign_auth(&kp, &digest, &nonce);
        assert_eq!(
            verify_auth(None, &digest, &nonce, &signature),
            Err(RejectReason::NoBoundKey)
        );
    }

    #[test]
    fn fingerprint_groups_are_always_four_digits() {
        // 指纹是操作者要逐字对读的：每组必须固定 4 位，不能被前导零吃掉。
        for _ in 0..32 {
            let kp = setup();
            let fp = fingerprint(&kp.pk);
            let groups: Vec<&str> = fp.split('-').collect();
            assert_eq!(groups.len(), 8, "fingerprint shape: {fp}");
            for group in groups {
                assert_eq!(group.len(), 4, "group {group} in {fp}");
                assert!(group
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase()));
            }
        }
        // 上面那个循环只有在某个摘要字节恰好是 0x00 时才会暴露 `{:02X}` 吃前导零的问题
        // （每组 1/256，256 组仍有 \u{2248}37% 概率全躲过）。直接喂一个以 0x00 开头的摘要，
        // 让回归确定性地失败。
        let digest: Vec<u8> = std::iter::once(0u8).chain(1u8..16).collect();
        assert_eq!(
            fingerprint_from_digest(&digest),
            "0001-0203-0405-0607-0809-0A0B-0C0D-0E0F"
        );
    }

    #[tokio::test]
    async fn a_different_protocol_version_is_refused() {
        let master = setup();
        let (host_pk, host_sk) = sign::gen_keypair();
        let digest = transcript();
        let mut pinned = PinnedHosts::default();
        let (mut master_stream, mut service_stream) = stream_pair().await;
        // Service 侧先发一个「未来版本」的挑战，Master 必须拒绝而不是当成 v1。
        let nonce = new_nonce();
        let mut challenge = crate::rendezvous_proto::RendezvousMessage::new();
        challenge.set_auth_nonce(crate::rendezvous_proto::AuthNonce {
            nonce: bytes::Bytes::copy_from_slice(&nonce),
            version: AUTH_VERSION + 1,
            host_pk: bytes::Bytes::copy_from_slice(&host_pk.0),
            host_proof: sign_host(&host_sk, &digest, &nonce).into(),
            ..Default::default()
        });
        service_stream.send(&challenge).await.expect("send challenge");
        let result = answer_challenge(
            &mut master_stream,
            &master,
            &digest,
            &mut pinned,
            "127.0.0.1:21118",
            true,
        )
        .await;
        assert_eq!(result, Err(RejectReason::VersionMismatch));
    }

    #[test]
    fn pin_outcomes() {
        let host = setup();
        let mut pinned = PinnedHosts::default();
        assert_eq!(pinned.pin_or_check("10.0.0.5:21118", &host.pk), PinOutcome::New);
        assert_eq!(
            pinned.pin_or_check("10.0.0.5:21118", &host.pk),
            PinOutcome::Verified
        );
        let recorded = pinned.get("10.0.0.5:21118").expect("pinned").clone();
        assert!(!recorded.verified);
        assert_eq!(recorded.fingerprint, fingerprint(&host.pk));

        let other = setup();
        assert_eq!(
            pinned.pin_or_check("10.0.0.5:21118", &other.pk),
            PinOutcome::Changed
        );
        // 指纹变化时旧记录保持不动。
        assert_eq!(pinned.get("10.0.0.5:21118"), Some(&recorded));

        assert!(pinned.mark_verified("10.0.0.5:21118"));
        assert!(pinned.get("10.0.0.5:21118").expect("pinned").verified);
        assert!(pinned.forget("10.0.0.5:21118"));
        assert!(pinned.is_empty());
    }

    #[test]
    fn pinned_hosts_roundtrip_through_json() {
        let host = setup();
        let dir = std::env::temp_dir().join("landesk-keyauth-test");
        let path = dir.join("pinned.json");
        let mut pinned = PinnedHosts::default();
        pinned.pin_or_check("10.0.0.9:21118", &host.pk);
        pinned.save(&path).expect("save");
        let loaded = PinnedHosts::load(&path).expect("load");
        assert_eq!(loaded.get("10.0.0.9:21118"), pinned.get("10.0.0.9:21118"));
        assert_eq!(
            loaded.get("10.0.0.9:21118").expect("pinned").host_pk,
            to_hex(&host.pk.0)
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn keypair_roundtrip() {
        let kp = setup();
        let restored = GlobalKeyPair::from_secret_bytes(kp.secret_bytes()).expect("restore");
        assert_eq!(restored.pk.0, kp.pk.0);
    }

    #[test]
    fn reject_log_line_shape() {
        assert_eq!(
            reject_log_line(RejectReason::BadSignature, "10.0.0.5:5555"),
            "landesk: reject bad-signature from 10.0.0.5:5555"
        );
    }

    async fn stream_pair() -> (crate::tcp::FramedStream, crate::tcp::FramedStream) {
        let listener = crate::tcp::new_listener("127.0.0.1:0", false)
            .await
            .expect("listen");
        let addr = listener.local_addr().expect("local addr");
        let dialing = tokio::spawn(async move {
            crate::tcp::FramedStream::new(addr.to_string(), None, 5_000)
                .await
                .expect("connect")
        });
        let (stream, _) = listener.accept().await.expect("accept");
        stream.set_nodelay(true).ok();
        let service = crate::tcp::FramedStream::from(stream, addr);
        (dialing.await.expect("join"), service)
    }

    #[tokio::test]
    async fn handshake_over_a_real_socket_accepts_the_right_key() {
        let kp = setup();
        let (host_pk, host_sk) = sign::gen_keypair();
        let digest = transcript();
        let mut pinned = PinnedHosts::default();
        let (mut master, mut service) = stream_pair().await;
        let (verified, answered) = tokio::join!(
            challenge_and_verify(&mut service, Some(&kp.pk), &host_pk, &host_sk, &digest),
            answer_challenge(&mut master, &kp, &digest, &mut pinned, "127.0.0.1:21118", true)
        );
        assert_eq!(verified, Ok(AuthRole::Controller));
        // 首次连接：记录指纹，标记未验证。
        assert_eq!(answered, Ok(PinOutcome::New));
        assert_eq!(pinned.len(), 1);
        let recorded = pinned.get("127.0.0.1:21118").expect("pinned");
        assert!(!recorded.verified);
        assert_eq!(recorded.fingerprint, fingerprint(&host_pk));
    }

    #[tokio::test]
    async fn handshake_over_a_real_socket_refuses_the_wrong_key() {
        let master_key = setup();
        let service_key = setup();
        let (host_pk, host_sk) = sign::gen_keypair();
        let digest = transcript();
        let mut pinned = PinnedHosts::default();
        let (mut master, mut service) = stream_pair().await;
        let (verified, answered) = tokio::join!(
            challenge_and_verify(
                &mut service,
                Some(&service_key.pk),
                &host_pk,
                &host_sk,
                &digest
            ),
            answer_challenge(
                &mut master,
                &master_key,
                &digest,
                &mut pinned,
                "127.0.0.1:21118",
                true
            )
        );
        // 客户端现在会在证明里带上自己的公钥：服务端能直接判定“这把身份不在这台机器允许的集合里”。
        assert_eq!(verified, Err(RejectReason::KeyNotAllowed));
        // Master 也必须知道自己的证明没被接受，而且现在能知道**具体原因**（服务端会回 AuthErr）。
        assert_eq!(answered, Err(RejectReason::KeyNotAllowed));
    }

    #[tokio::test]
    async fn a_changed_host_fingerprint_stops_the_connection() {
        let kp = setup();
        let (host_pk, host_sk) = sign::gen_keypair();
        let (other_pk, _) = sign::gen_keypair();
        let digest = transcript();
        let mut pinned = PinnedHosts::default();
        // 地址簿里已经记了另一把主机公钥。
        pinned.pin_or_check("127.0.0.1:21118", &other_pk);
        let (mut master, mut service) = stream_pair().await;
        let (verified, answered) = tokio::join!(
            challenge_and_verify(&mut service, Some(&kp.pk), &host_pk, &host_sk, &digest),
            answer_challenge(&mut master, &kp, &digest, &mut pinned, "127.0.0.1:21118", true)
        );
        assert_eq!(answered, Err(RejectReason::HostFingerprintMismatch));
        // Master 不发证明，Service 只能等到超时。
        assert_eq!(verified, Err(RejectReason::NoProofYet));
        // 旧记录不动。
        assert_eq!(
            pinned.get("127.0.0.1:21118").expect("pinned").host_pk,
            {
                let mut s = String::new();
                for b in other_pk.0 {
                    s.push_str(&format!("{b:02x}"));
                }
                s
            }
        );
    }

    #[tokio::test]
    async fn a_second_connection_is_verified() {
        let kp = setup();
        let (host_pk, host_sk) = sign::gen_keypair();
        let digest = transcript();
        let mut pinned = PinnedHosts::default();
        for expected in [PinOutcome::New, PinOutcome::Verified] {
            let (mut master, mut service) = stream_pair().await;
            let (verified, answered) = tokio::join!(
                challenge_and_verify(&mut service, Some(&kp.pk), &host_pk, &host_sk, &digest),
                answer_challenge(&mut master, &kp, &digest, &mut pinned, "127.0.0.1:21118", true)
            );
            assert_eq!(verified, Ok(AuthRole::Controller));
            assert_eq!(answered, Ok(expected));
        }
    }

    #[tokio::test]
    async fn a_master_can_connect_as_a_viewer() {
        // 多台 Master 共用一把私钥：来看的那台不该抢走操作权。
        let kp = setup();
        let (host_pk, host_sk) = sign::gen_keypair();
        let digest = transcript();
        let mut pinned = PinnedHosts::default();
        let (mut master, mut service) = stream_pair().await;
        let (verified, answered) = tokio::join!(
            challenge_and_verify(&mut service, Some(&kp.pk), &host_pk, &host_sk, &digest),
            answer_challenge(
                &mut master,
                &kp,
                &digest,
                &mut pinned,
                "127.0.0.1:21118",
                false
            )
        );
        assert_eq!(verified, Ok(AuthRole::Viewer));
        assert_eq!(answered, Ok(PinOutcome::New));
    }

    #[tokio::test]
    async fn unbound_service_refuses_without_talking() {
        let kp = setup();
        let (host_pk, host_sk) = sign::gen_keypair();
        let digest = transcript();
        let mut pinned = PinnedHosts::default();
        let (mut master, mut service) = stream_pair().await;
        let (verified, answered) = tokio::join!(
            challenge_and_verify(&mut service, None, &host_pk, &host_sk, &digest),
            tokio::time::timeout(
                std::time::Duration::from_millis(500),
                answer_challenge(&mut master, &kp, &digest, &mut pinned, "127.0.0.1:21118", true)
            )
        );
        assert_eq!(verified, Err(RejectReason::NoBoundKey));
        // 服务端没发挑战，Master 只能等到超时。
        assert!(answered.is_err(), "master must not receive a challenge");
    }
}
