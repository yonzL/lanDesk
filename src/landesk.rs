//! landesk 在 Service 侧的配置与鉴权执行点。
//!
//! 设计见 `architecture.md` 与 `.thinkrail/context/TASK-keyauth-handshake.md`：Service 在密钥交换
//! 之后、任何会话消息之前要求 Master 证明自己持有全局私钥；这把公钥在安装时绑定，运行期只读。
//!
//! 只有显式给出 `--landesk-listen` / `--landesk-auth-key` 时本模块才生效，未配置时保持上游行为。

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub mod cm;
pub mod net;
pub mod session;

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
    /// 把当前参数写成可执行文件同目录的 `landesk.json`（服务开机自启动时读它）。
    pub save_config: bool,
    /// Master 侧：以观察者身份连接（只看画面，不抢操作权）。
    pub view: bool,
    /// Service 自检：把配置、端口、主机指纹与需要人工确认的事项列出来。
    pub doctor: bool,
    /// 命令行扇出：`--landesk-push a,b,c` + `--landesk-to <远端目录>` + `--landesk-files f1,f2`。
    pub push: Option<Vec<String>>,
    pub push_to: Option<String>,
    pub push_files: Option<Vec<String>>,
    /// 允许连入的来源网段（CIDR）；空表示只用 `net::DEFAULT_NETWORKS` 里的私有网段。
    pub allowed_networks: Vec<String>,
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
    let mut save_config = false;
    let mut doctor = false;
    let mut push = None;
    let mut push_to = None;
    let mut push_files = None;
    let mut view = false;
    let mut allowed_networks: Vec<String> = Vec::new();
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
            "--landesk-auth-key-file" => {
                let Some(value) = iter.next() else {
                    bail!("--landesk-auth-key-file needs a path to public.key");
                };
                auth_pk = Some(read_public_key_file(std::path::Path::new(&value))?);
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
            "--landesk-save-config" => save_config = true,
            "--landesk-doctor" => doctor = true,
            "--landesk-allow-network" => {
                let Some(value) = iter.next() else {
                    bail!("--landesk-allow-network needs a comma separated list of CIDR ranges");
                };
                allowed_networks.extend(split_list(&value));
            }
            "--landesk-push" => {
                let Some(value) = iter.next() else {
                    bail!("--landesk-push needs a comma separated list of addr:port");
                };
                push = Some(split_list(&value));
            }
            "--landesk-to" => {
                let Some(value) = iter.next() else {
                    bail!("--landesk-to needs a remote directory");
                };
                push_to = Some(value);
            }
            "--landesk-files" => {
                let Some(value) = iter.next() else {
                    bail!("--landesk-files needs a comma separated list of local paths");
                };
                push_files = Some(split_list(&value));
            }
            "--landesk-view" => view = true,
            _ => rest.push(arg),
        }
    }
    *args = rest;
    // 命令行没给 landesk 参数时，读可执行文件同目录的 landesk.json。
    // 这一条让“装成服务、开机自启”成立：服务派生的 --server 子进程不带这些参数。
    // 必须排在 public.key 自动读取**之前**：那个文件常年存在，先读它会让整份 JSON 被跳过。
    if listen_port.is_none() && auth_pk.is_none() {
        if let Some((port, pk, nets)) = load_config_file() {
            listen_port = port;
            auth_pk = pk;
            if allowed_networks.is_empty() {
                allowed_networks = nets;
            }
        }
    }

    // 还是没有公钥时，默认读程序同目录的 public.key——装 Service 时只要把这个文件拷过去。
    if auth_pk.is_none() {
        let candidate = exe_dir().join("public.key");
        if candidate.is_file() {
            if let Ok(pk) = read_public_key_file(&candidate) {
                auth_pk = Some(pk);
                log::info!(
                    "landesk: using the public key from {}",
                    candidate.display()
                );
            }
        }
    }

    if listen_port.is_some()
        || auth_pk.is_some()
        || connect.is_some()
        || genkey.is_some()
        || book_import.is_some()
        || book_export.is_some()
        || book_list
        || book_verify.is_some()
        || save_config
        || view
        || doctor
        || push.is_some()
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
            save_config,
            view,
            doctor,
            push,
            push_to,
            push_files,
            allowed_networks,
        });
    }
    Ok(())
}

/// `landesk.json` 的形状：服务开机自启时读的那份配置。
#[derive(Debug, Default, serde_derive::Serialize, serde_derive::Deserialize)]
struct ConfigFile {
    #[serde(default)]
    listen_port: Option<u16>,
    #[serde(default)]
    auth_key: Option<String>,
    #[serde(default)]
    allowed_networks: Vec<String>,
}

fn config_file_path() -> PathBuf {
    exe_dir().join("landesk.json")
}

fn load_config_file() -> Option<(Option<u16>, Option<sign::PublicKey>, Vec<String>)> {
    let text = std::fs::read_to_string(config_file_path()).ok()?;
    let file: ConfigFile = serde_json::from_str(&text).ok()?;
    let auth_pk = file
        .auth_key
        .as_deref()
        .and_then(|key| parse_public_key(key).ok());
    if file.listen_port.is_none() && auth_pk.is_none() {
        None
    } else {
        Some((file.listen_port, auth_pk, file.allowed_networks))
    }
}

/// Service 自检：把能被机器判定的事实直接给结论，剩下需要人做的事也列出来。
pub fn run_doctor() -> ResultType<()> {
    let dir = exe_dir();
    println!("lanDesk 自检（只读，不改任何东西）");
    println!("程序目录：{}", dir.display());

    let config_path = config_file_path();
    let config = config();
    match (config_path.is_file(), config) {
        (true, Some(config)) => {
            let port = config
                .listen_port
                .map(|port| port.to_string())
                .unwrap_or_else(|| "（未设置）".to_owned());
            println!("  [ ok ] landesk.json 存在：{}", config_path.display());
            println!("  [info] 监听端口：{port}");
        }
        (true, None) => println!(
            "  [warn] {} 存在但读不出来（格式错？）",
            config_path.display()
        ),
        (false, Some(_)) => println!("  [info] 用命令行参数启动，没有 landesk.json（服务自启需要它）"),
        (false, None) => println!(
            "  [fail] 既没有命令行参数也没有 landesk.json：这台机器不会监听。\n         \
             先跑：landesk-server.exe --landesk-save-config --landesk-listen 21118 --landesk-auth-key <公钥>"
        ),
    }

    match config.and_then(|config| config.auth_pk.as_ref()) {
        Some(pk) => println!("  [ ok ] 已绑定公钥：{}", public_key_hex(pk)),
        None => println!("  [fail] 没有绑定公钥：所有连接都会被拒（no-bound-key）"),
    }

    match host_keypair() {
        Some((pk, _)) => {
            println!("  [info] 本机主机指纹：{}", keyauth::fingerprint(&pk));
            println!("         对端首次连接会记下它，需要人工核对后标为已验证");
        }
        None => println!("  [fail] 读不到主机密钥（上游配置里的 key_pair 无效）"),
    }

    if let Some(port) = config.and_then(|config| config.listen_port) {
        let addr = format!("0.0.0.0:{port}");
        match std::net::TcpListener::bind(&addr) {
            Ok(_) => println!("  [ ok ] 端口 {port} 现在可以绑定"),
            Err(err) => println!(
                "  [warn] 端口 {port} 现在绑不了：{err}（如果 Service 已经在跑，这是正常的）"
            ),
        }
    }

    println!("\n还需要人工确认：");
    println!("  - 防火墙放行 TCP 端口（管理员）：");
    println!(
        "      netsh advfirewall firewall add rule name=\"lanDesk\" dir=in action=allow protocol=TCP localport=21118"
    );
    println!("  - 想开机自启：landesk-server.exe --install-service（管理员）");
    println!("  - 目标机停在登录界面时也能连：Service 需要以服务/SYSTEM 身份运行");
    Ok(())
}

/// 把当前配置写到可执行文件同目录，返回文件路径。
pub fn save_config_file(config: &LandeskConfig) -> ResultType<PathBuf> {
    let path = config_file_path();
    let file = ConfigFile {
        listen_port: config.listen_port,
        auth_key: config.auth_pk.as_ref().map(public_key_hex),
        allowed_networks: config.allowed_networks.clone(),
    };
    if file.listen_port.is_none() && file.auth_key.is_none() {
        bail!("--landesk-save-config 需要同时给 --landesk-listen 或 --landesk-auth-key");
    }
    let text = serde_json::to_string_pretty(&file)
        .map_err(|err| hbb_common::anyhow::anyhow!("serializing landesk.json: {err}"))?;
    std::fs::write(&path, text)
        .map_err(|err| hbb_common::anyhow::anyhow!("writing {}: {err}", path.display()))?;
    Ok(path)
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

fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|item| item.trim().to_owned())
        .filter(|item| !item.is_empty())
        .collect()
}

/// 命令行扇出：把文件推给多台，等全部结束再把结果逐台打出来。
pub fn run_push(config: &LandeskConfig) -> ResultType<bool> {
    let (Some(peers), Some(files)) = (config.push.clone(), config.push_files.clone()) else {
        return Ok(false);
    };
    let Some(remote_dir) = config.push_to.clone() else {
        bail!("--landesk-push 需要同时给 --landesk-to <远端目录>");
    };
    if peers.is_empty() || files.is_empty() {
        bail!("--landesk-push 与 --landesk-files 都不能为空");
    }
    println!(
        "把 {} 个文件推给 {} 台机器的 {}：",
        files.len(),
        peers.len(),
        remote_dir
    );
    for file in &files {
        if !std::path::Path::new(file).exists() {
            bail!("本机文件不存在：{file}");
        }
        println!("  - {file}");
    }
    let distribution = crate::landesk::session::distribute(
        peers,
        config.book_path(),
        config.key_path(),
        files,
        remote_dir,
    );
    while !distribution.finished() {
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    let mut failed = 0;
    for report in distribution.snapshot() {
        if report.failed > 0 {
            failed += 1;
            println!(
                "  [失败] {}：{} / {}，原因：{}",
                report.peer,
                report.failed,
                report.total,
                report.last_error.unwrap_or_else(|| "未知".to_owned())
            );
        } else {
            println!("  [ ok ] {}：{}/{} 完成", report.peer, report.done, report.total);
        }
    }
    if failed > 0 {
        bail!("{failed} 台机器分发失败");
    }
    Ok(true)
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

/// 这个进程是不是 landesk（而不是上游 RustDesk）：由产品名判定。
pub fn is_landesk() -> bool {
    hbb_common::config::APP_NAME.read().unwrap().eq("lanDesk")
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

/// 生成一对全局密钥：私钥写到 `private_path`，公钥作为 `public.key` 写在同目录。
///
/// 公钥交给 Service（安装时选这个文件），私钥只留在控制端。返回公钥文件路径。
pub fn generate_key_pair(private_path: &std::path::Path) -> ResultType<std::path::PathBuf> {
    let keypair = keyauth::GlobalKeyPair::generate();
    keypair.save(private_path)?;
    let public_path = private_path.with_file_name("public.key");
    std::fs::write(
        &public_path,
        format!("{}\n", public_key_hex(&keypair.pk)),
    )
    .map_err(|err| hbb_common::anyhow::anyhow!("writing {}: {err}", public_path.display()))?;
    Ok(public_path)
}

/// 读一个公钥文件（小写/大写十六进制都行，允许前后空白）。
pub fn read_public_key_file(path: &std::path::Path) -> ResultType<sign::PublicKey> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| hbb_common::anyhow::anyhow!("reading {}: {err}", path.display()))?;
    parse_public_key(text.trim())
}

/// Service 侧的鉴权执行点：在密钥交换完成后调用，失败即断开。
///
/// 日志一行的形状由 `keyauth::reject_log_line` 定义，含原因与来源地址。
pub async fn enforce(
    stream: &mut hbb_common::Stream,
    transcript: &[u8; 32],
    addr: SocketAddr,
) -> ResultType<keyauth::AuthRole> {
    let Some(config) = config() else {
        return Ok(keyauth::AuthRole::Controller);
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
        Ok(role) => Ok(role),
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
