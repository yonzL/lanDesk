//! 密钥鉴权：Master 用全局私钥证明身份，Service 用绑定的公钥验签；Master 另用「钉住的主机公钥」
//! 确认对面是地址簿里那台机器。
//!
//! 通道本身沿用 [`crate::tcp`] 的密钥交换，这里只补两件事：本模块的 `AuthNonce` / `AuthProof`
//! 承担 Master → Service 的身份，Service → Master 的身份由密钥交换里已有的主机签名承担。
//! 设计与取舍见 `architecture.md` 与 `.thinkrail/context/TASK-keyauth-handshake.md`。

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context as _, Result};
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
    let mut payload = Vec::with_capacity(AUTH_DOMAIN.len() + transcript.len() + nonce.len());
    payload.extend_from_slice(AUTH_DOMAIN);
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
    /// 证明到达之前就收到了会话消息。
    NoProofYet,
}

impl RejectReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NoBoundKey => "no-bound-key",
            Self::BadSignature => "bad-signature",
            Self::HostFingerprintMismatch => "host-fingerprint-mismatch",
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
}
