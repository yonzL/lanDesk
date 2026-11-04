//! landesk 在 Service 侧的配置与鉴权执行点。
//!
//! 设计见 `architecture.md` 与 `.thinkrail/context/TASK-keyauth-handshake.md`：Service 在密钥交换
//! 之后、任何会话消息之前要求 Master 证明自己持有全局私钥；这把公钥在安装时绑定，运行期只读。
//!
//! 只有显式给出 `--landesk-listen` / `--landesk-auth-key` 时本模块才生效，未配置时保持上游行为。

use std::net::SocketAddr;
use std::path::PathBuf;
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
    /// Master 侧：要拨号的 `地址:端口`。
    pub connect: Option<String>,
    /// Master 侧：地址簿文件；缺省是可执行文件同目录的 `addressbook.json`。
    pub book: Option<PathBuf>,
    /// Master 侧：全局私钥文件；缺省是可执行文件同目录的 `private.key`。
    pub key: Option<PathBuf>,
    /// 生成一对全局密钥对并写到该路径，打印公钥供 Service 绑定。
    pub genkey: Option<PathBuf>,
    /// 导入地址簿：`.csv` 合并条目，`.json` 整份替换。
    pub book_import: Option<PathBuf>,
    /// 导出地址簿：按扩展名写 `.csv` 或 `.json`。
    pub book_export: Option<PathBuf>,
    /// 列出地址簿条目。
    pub book_list: bool,
    /// 把某个 `地址:端口` 标为已验证（操作者核过指纹之后）。
    pub book_verify: Option<String>,
}

impl LandeskConfig {
    pub fn book_path(&self) -> PathBuf {
        self.book.clone().unwrap_or_else(|| exe_dir().join("addressbook.json"))
    }

    pub fn key_path(&self) -> PathBuf {
        self.key.clone().unwrap_or_else(|| exe_dir().join("private.key"))
    }
}

/// 可执行文件所在目录：地址簿与密钥的默认落脚点。
pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

static CONFIG: OnceLock<LandeskConfig> = OnceLock::new();

/// 从已展开的参数里取走 landesk 自己的开关，剩下的留给上游的参数解析。
///
/// 支持的形式：`--landesk-listen 21118`、`--landesk-auth-key <64 位十六进制>`；
/// 不带值的 `--landesk-listen` 表示用默认端口。
pub fn take_from_args(args: &mut Vec<String>) -> ResultType<()> {
    let mut listen_port = None;
    let mut auth_pk = None;
    let mut connect = None;
    let mut book = None;
    let mut key = None;
    let mut genkey = None;
    let mut book_import = None;
    let mut book_export = None;
    let mut book_list = false;
    let mut book_verify = None;
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
            "--landesk-connect" => {
                let Some(value) = iter.next() else {
                    bail!("--landesk-connect needs an address:port");
                };
                connect = Some(value);
            }
            "--landesk-book" => {
                let Some(value) = iter.next() else {
                    bail!("--landesk-book needs a path");
                };
                book = Some(PathBuf::from(value));
            }
            "--landesk-key" => {
                let Some(value) = iter.next() else {
                    bail!("--landesk-key needs a path");
                };
                key = Some(PathBuf::from(value));
            }
            "--landesk-genkey" => {
                let Some(value) = iter.next() else {
                    bail!("--landesk-genkey needs a path");
                };
                genkey = Some(PathBuf::from(value));
            }
            "--landesk-book-import" => {
                let Some(value) = iter.next() else {
                    bail!("--landesk-book-import needs a path");
                };
                book_import = Some(PathBuf::from(value));
            }
            "--landesk-book-export" => {
                let Some(value) = iter.next() else {
                    bail!("--landesk-book-export needs a path");
                };
                book_export = Some(PathBuf::from(value));
            }
            "--landesk-book-list" => book_list = true,
            "--landesk-book-verify" => {
                let Some(value) = iter.next() else {
                    bail!("--landesk-book-verify needs an address:port");
                };
                book_verify = Some(value);
            }
            _ => rest.push(arg),
        }
    }
    *args = rest;
    if listen_port.is_some()
        || auth_pk.is_some()
        || connect.is_some()
        || genkey.is_some()
        || book_import.is_some()
        || book_export.is_some()
        || book_list
        || book_verify.is_some()
    {
        CONFIG.get_or_init(|| LandeskConfig {
            listen_port,
            auth_pk,
            connect,
            book,
            key,
            genkey,
            book_import,
            book_export,
            book_list,
            book_verify,
        });
    }
    Ok(())
}

/// 地址簿的读/导入/导出命令。都在拿到全量数据、校验通过之后才落盘。
pub fn run_book_command(config: &LandeskConfig) -> ResultType<()> {
    use crate::addressbook::AddressBook;

    let book_path = config.book_path();
    if let Some(source) = &config.book_import {
        let is_csv = source
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("csv"))
            .unwrap_or(false);
        if is_csv {
            let text = std::fs::read_to_string(source)
                .map_err(|err| hbb_common::anyhow::anyhow!("reading {}: {err}", source.display()))?;
            let mut book = AddressBook::load(&book_path)?;
            let count = book.merge_csv(&text)?;
            book.save(&book_path)?;
            println!("导入 {count} 条，地址簿现有 {} 条：{}", book.entries().len(), book_path.display());
        } else {
            let book = AddressBook::import(source, &book_path)?;
            println!("已导入整份地址簿（{} 条）：{}", book.entries().len(), book_path.display());
        }
    }

    if let Some(target) = &config.book_export {
        let book = AddressBook::load(&book_path)?;
        let is_csv = target
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("csv"))
            .unwrap_or(false);
        if is_csv {
            std::fs::write(target, book.to_csv())
                .map_err(|err| hbb_common::anyhow::anyhow!("writing {}: {err}", target.display()))?;
        } else {
            book.export(target)?;
        }
        println!("已导出 {} 条到 {}", book.entries().len(), target.display());
    }

    if let Some(addr) = &config.book_verify {
        let mut book = AddressBook::load(&book_path)?;
        if !book.mark_verified(addr) {
            bail!("{addr} 不在指纹库里（先连一次再标记）");
        }
        book.save(&book_path)?;
        println!(
            "已把 {addr} 标为已验证（{}）",
            book.fingerprint_of(addr).unwrap_or("?")
        );
    }

    if config.book_list {
        let book = AddressBook::load(&book_path)?;
        if book.entries().is_empty() {
            println!("地址簿为空：{}", book_path.display());
        }
        for entry in book.entries() {
            let state = match book.pins().get(&entry.addr) {
                None => "未连接过".to_owned(),
                Some(host) if host.verified => format!("已验证 {}", host.fingerprint),
                Some(host) => format!("未验证 {}", host.fingerprint),
            };
            println!(
                "{}/{}\t{}\t{}\t{state}",
                entry.group, entry.subgroup, entry.name, entry.addr
            );
        }
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

/// landesk 模式是否接管了网络入口：配置了监听端口就不该再启动上游的 rendezvous。
pub fn takes_over_network() -> bool {
    config()
        .map(|config| config.listen_port.is_some())
        .unwrap_or(false)
}

/// 公钥的十六进制文本，与 `--landesk-auth-key` 接受的形式一致。
pub fn public_key_hex(pk: &sign::PublicKey) -> String {
    pk.0.iter().map(|byte| format!("{byte:02x}")).collect()
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
        hbb_common::Stream::Tcp(tcp) => match host_keypair() {
            Some((host_pk, host_sk)) => {
                keyauth::challenge_and_verify(
                    tcp,
                    config.auth_pk.as_ref(),
                    &host_pk,
                    &host_sk,
                    transcript,
                )
                .await
            }
            // 没有主机密钥就无法向 Master 证明“我是哪台机器”，宁可不接。
            None => Err(RejectReason::BadHostProof),
        },
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

/// Service 的主机密钥对，来自上游配置（与密钥交换用的是同一对）。
fn host_keypair() -> Option<(sign::PublicKey, sign::SecretKey)> {
    let (sk, pk) = hbb_common::config::Config::get_key_pair();
    let pk = sign::PublicKey::from_slice(&pk)?;
    let sk = sign::SecretKey::from_slice(&sk)?;
    Some((pk, sk))
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
