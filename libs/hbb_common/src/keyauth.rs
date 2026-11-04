//! 密钥鉴权：Master 用全局私钥证明身份，Service 用绑定的公钥验签；Master 另用「钉住的主机公钥」
//! 确认对面是地址簿里那台机器。
//!
//! 通道本身沿用 [`crate::tcp`] 的密钥交换，这里只补两件事：本模块的 `AuthNonce` / `AuthProof`
//! 承担 Master → Service 的身份，Service → Master 的身份由密钥交换里已有的主机签名承担。
//! 设计与取舍见 `architecture.md` 与 `.thinkrail/context/TASK-keyauth-handshake.md`。

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
        Self::from_secret_bytes(&bytes)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        std::fs::write(path, to_hex(self.secret_bytes()))
            .with_context(|| format!("writing private key {}", path.display()))
    }
}

/// 主机公钥的指纹：SHA-256 的前 16 字节，写成大写十六进制并按 4 位分组。
pub fn fingerprint_bytes(pk: &[u8]) -> String {
    let digest = Sha256::digest(pk);
    let groups: Vec<String> = digest[..16]
        .chunks(2)
        .map(|pair| format!("{:02X}", (u16::from(pair[0]) << 8) | u16::from(pair[1])))
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
pub fn verify_auth(
    bound_pk: Option<&sign::PublicKey>,
    transcript: &[u8; 32],
    nonce: &[u8],
    signature: &[u8],
) -> std::result::Result<(), RejectReason> {
    let Some(pk) = bound_pk else {
        return Err(RejectReason::NoBoundKey);
    };
    if nonce.len() != NONCE_LEN {
        return Err(RejectReason::BadSignature);
    }
    match sign::verify(signature, pk) {
        Ok(message) if message == signed_payload(transcript, nonce) => Ok(()),
        _ => Err(RejectReason::BadSignature),
    }
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
            Some(known) if known.host_pk == pk => PinOutcome::Verified,
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
) -> std::result::Result<(), RejectReason> {
    let Some(pk) = bound_pk else {
        return Err(RejectReason::NoBoundKey);
    };
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
        return Err(RejectReason::NoProofYet);
    }
    match stream.next_timeout(AUTH_TIMEOUT_MS).await {
        Some(Ok(bytes)) => match crate::rendezvous_proto::RendezvousMessage::parse_from_bytes(&bytes)
        {
            Ok(crate::rendezvous_proto::RendezvousMessage {
                union: Some(crate::rendezvous_proto::rendezvous_message::Union::AuthProof(proof)),
                ..
            }) => match verify_auth(Some(pk), transcript, &nonce, &proof.signature) {
                Ok(()) => {
                    // 显式告诉对面“过了”：否则 Master 分不清“被拒”与“连接没了”。
                    let mut ok = crate::rendezvous_proto::RendezvousMessage::new();
                    ok.set_auth_ok(crate::rendezvous_proto::AuthOk::default());
                    if stream.send(&ok).await.is_err() {
                        return Err(RejectReason::NoProofYet);
                    }
                    Ok(())
                }
                Err(reason) => Err(reason),
            },
            _ => Err(RejectReason::NoProofYet),
        },
        _ => Err(RejectReason::NoProofYet),
    }
}

/// Master 侧：等对面的挑战，先验证主机身份（并按指纹库做 TOFU），再用全局私钥回证明。
///
/// 返回指纹库对这台机器的结论：`New` 表示首次连接、已记录并标为未验证。
pub async fn answer_challenge(
    stream: &mut crate::tcp::FramedStream,
    kp: &GlobalKeyPair,
    transcript: &[u8; 32],
    pinned: &mut PinnedHosts,
    addr: &str,
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
        ..Default::default()
    });
    stream.send(&proof).await.map_err(|_| RejectReason::NoProofYet)?;
    // 等 Service 的明示：没收到回执就当没通过。
    match stream.next_timeout(AUTH_TIMEOUT_MS).await {
        Some(Ok(bytes)) => match crate::rendezvous_proto::RendezvousMessage::parse_from_bytes(&bytes)
        {
            Ok(crate::rendezvous_proto::RendezvousMessage {
                union: Some(crate::rendezvous_proto::rendezvous_message::Union::AuthOk(_)),
                ..
            }) => Ok(outcome),
            _ => Err(RejectReason::BadSignature),
        },
        _ => Err(RejectReason::BadSignature),
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
            answer_challenge(&mut master, &kp, &digest, &mut pinned, "127.0.0.1:21118")
        );
        assert_eq!(verified, Ok(()));
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
                "127.0.0.1:21118"
            )
        );
        assert_eq!(verified, Err(RejectReason::BadSignature));
        // Master 也必须知道自己的证明没被接受，不能当成功。
        assert_eq!(answered, Err(RejectReason::BadSignature));
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
            answer_challenge(&mut master, &kp, &digest, &mut pinned, "127.0.0.1:21118")
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
                answer_challenge(&mut master, &kp, &digest, &mut pinned, "127.0.0.1:21118")
            );
            assert_eq!(verified, Ok(()));
            assert_eq!(answered, Ok(expected));
        }
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
                answer_challenge(&mut master, &kp, &digest, &mut pinned, "127.0.0.1:21118")
            )
        );
        assert_eq!(verified, Err(RejectReason::NoBoundKey));
        // 服务端没发挑战，Master 只能等到超时。
        assert!(answered.is_err(), "master must not receive a challenge");
    }
}
