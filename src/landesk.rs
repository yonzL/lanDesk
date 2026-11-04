//! landesk 在 Service 侧的配置与鉴权执行点。
//!
//! 设计见 `architecture.md` 与 `.thinkrail/context/TASK-keyauth-handshake.md`：Service 在密钥交换
//! 之后、任何会话消息之前要求 Master 证明自己持有全局私钥；这把公钥在安装时绑定，运行期只读。
//!
//! 只有显式给出 `--landesk-listen` / `--landesk-auth-key` 时本模块才生效，未配置时保持上游行为。

use std::net::SocketAddr;
use std::sync::OnceLock;

use hbb_common::keyauth::{self, RejectReason};
use hbb_common::log;
use hbb_common::sodiumoxide::crypto::sign;
use hbb_common::{bail, ResultType};

/// 直连监听的默认端口。
pub const DEFAULT_LISTEN_PORT: u16 = 21118;

#[derive(Debug, Clone)]
pub struct LandeskConfig {
    /// 直连监听端口。给出 `--landesk-listen` 时 Service 会在该端口接受连接。
    pub listen_port: Option<u16>,
    /// 安装时绑定的全局公钥；`None` 表示这台 Service 还没绑定，一律拒绝。
    pub auth_pk: Option<sign::PublicKey>,
}

static CONFIG: OnceLock<LandeskConfig> = OnceLock::new();

/// 从已展开的参数里取走 landesk 自己的开关，剩下的留给上游的参数解析。
///
/// 支持的形式：`--landesk-listen 21118`、`--landesk-auth-key <64 位十六进制>`；
/// 不带值的 `--landesk-listen` 表示用默认端口。
pub fn take_from_args(args: &mut Vec<String>) -> ResultType<()> {
    let mut listen_port = None;
    let mut auth_pk = None;
    let mut rest = Vec::with_capacity(args.len());
    let incoming = std::mem::take(args);
    let mut iter = incoming.into_iter().peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--landesk-listen" => {
                listen_port = Some(match iter.peek().and_then(|v| v.parse::<u16>().ok()) {
                    Some(port) => {
                        iter.next();
                        port
                    }
                    None => DEFAULT_LISTEN_PORT,
                });
            }
            "--landesk-auth-key" => {
                let Some(value) = iter.next() else {
                    bail!("--landesk-auth-key needs a hex-encoded public key");
                };
                auth_pk = Some(parse_public_key(&value)?);
            }
            _ => rest.push(arg),
        }
    }
    *args = rest;
    if listen_port.is_some() || auth_pk.is_some() {
        CONFIG.get_or_init(|| LandeskConfig {
            listen_port,
            auth_pk,
        });
    }
    Ok(())
}

/// 公钥的文本形式与指纹一致：小写十六进制。
fn parse_public_key(text: &str) -> ResultType<sign::PublicKey> {
    let text = text.trim();
    let bytes: Option<Vec<u8>> = (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect();
    let Some(bytes) = bytes else {
        bail!("--landesk-auth-key is not valid hex");
    };
    match sign::PublicKey::from_slice(&bytes) {
        Some(pk) => Ok(pk),
        None => bail!(
            "--landesk-auth-key must be {} bytes, got {}",
            sign::PUBLICKEYBYTES,
            bytes.len()
        ),
    }
}

pub fn config() -> Option<&'static LandeskConfig> {
    CONFIG.get()
}

/// Service 侧的鉴权执行点：在密钥交换完成后调用，失败即断开。
///
/// 日志一行的形状由 `keyauth::reject_log_line` 定义，含原因与来源地址。
pub async fn enforce(
    stream: &mut hbb_common::Stream,
    transcript: &[u8; 32],
    addr: SocketAddr,
) -> ResultType<()> {
    let Some(config) = config() else {
        return Ok(());
    };
    let result = match stream {
        hbb_common::Stream::Tcp(tcp) => {
            keyauth::challenge_and_verify(tcp, config.auth_pk.as_ref(), transcript).await
        }
        // landesk 只走 TCP 直连；别的传输没有这条鉴权路径，一律当没证明处理。
        _ => Err(RejectReason::NoProofYet),
    };
    match result {
        Ok(()) => Ok(()),
        Err(reason) => {
            log::warn!("{}", keyauth::reject_log_line(reason, &addr.to_string()));
            bail!("landesk: authentication refused ({reason})")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hbb_common::sodiumoxide;

    fn hex_of(pk: &sign::PublicKey) -> String {
        pk.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn parses_a_hex_public_key() {
        let _ = sodiumoxide::init();
        let (pk, _) = sign::gen_keypair();
        let parsed = parse_public_key(&hex_of(&pk)).expect("parse");
        assert_eq!(parsed.0, pk.0);
    }

    #[test]
    fn refuses_a_wrong_length_key() {
        assert!(parse_public_key("00ff").is_err());
    }

    #[test]
    fn refuses_non_hex() {
        assert!(parse_public_key("zz").is_err());
    }

    #[test]
    fn leaves_upstream_args_untouched() {
        let mut args = vec!["--server".to_owned(), "--tray".to_owned()];
        take_from_args(&mut args).expect("parse");
        assert_eq!(args, vec!["--server".to_owned(), "--tray".to_owned()]);
    }
}
