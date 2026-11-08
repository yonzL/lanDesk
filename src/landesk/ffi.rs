//! Flutter（以及任何外部 UI）需要的 landesk 接口。
//!
//! 全部放在**默认构建**里：既能被 `cargo check`/测试覆盖，也让 `flutter_ffi.rs` 只留一层薄封装。
//! 返回字符串的约定统一为：成功 = JSON（或空串），失败 = 以 `error:` 开头的可读原因。
//!
//! 会话本身走**上游的 Flutter 会话链路**：窗口里的 `RemotePage` 直接 `start()`，Rust 侧的
//! 拨号器（`client::_start`）从地址簿与默认身份自己解析身份并走 keyauth——界面不需要任何预备调用。

use base::config::keys;
use serde_derive::Serialize;

use crate::addressbook::{AddressBook, Entry};

/// 地址簿里一条机器，连它的指纹状态一起给界面。
#[derive(Serialize)]
struct EntryView<'a> {
    group: &'a str,
    subgroup: &'a str,
    name: &'a str,
    addr: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<&'a str>,
    /// `verified` / `unverified` / `never`
    pin: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    fingerprint: Option<&'a str>,
}

fn book_path() -> std::path::PathBuf {
    crate::landesk::config()
        .map(|config| config.book_path())
        .unwrap_or_else(|| crate::landesk::exe_dir().join("addressbook.json"))
}

fn load_book() -> Result<AddressBook, String> {
    AddressBook::load(&book_path()).map_err(|err| format!("error:{err:#}"))
}

fn save_book(book: &AddressBook) -> Result<(), String> {
    book.save(&book_path())
        .map_err(|err| format!("error:{err:#}"))
}

/// 地址簿（组/子组/显示名/地址/身份/指纹状态）的 JSON 数组。
pub fn book_list() -> String {
    let book = match load_book() {
        Ok(book) => book,
        Err(err) => return err,
    };
    let pins = book.pins();
    let views: Vec<EntryView> = book
        .entries()
        .iter()
        .map(|entry: &Entry| {
            let pin = pins.get(&entry.addr);
            EntryView {
                group: &entry.group,
                subgroup: &entry.subgroup,
                name: &entry.name,
                addr: &entry.addr,
                key: entry.key.as_deref(),
                pin: match pin {
                    Some(host) if host.verified => "verified",
                    Some(_) => "unverified",
                    None => "never",
                },
                fingerprint: pin.map(|host| host.fingerprint.as_str()),
            }
        })
        .collect();
    serde_json::to_string(&views).unwrap_or_else(|err| format!("error:{err}"))
}

/// 新增或覆盖一条（按 `地址:端口`）。`key` 传空串表示用默认身份。
pub fn book_upsert(group: &str, subgroup: &str, name: &str, addr: &str, key: &str) -> String {
    if !addr.contains(':') {
        return "error:the address needs a port, e.g. 10.0.0.5:21118".to_owned();
    }
    let mut book = match load_book() {
        Ok(book) => book,
        Err(err) => return err,
    };
    book.upsert(Entry {
        group: group.to_owned(),
        subgroup: subgroup.to_owned(),
        name: name.to_owned(),
        addr: addr.to_owned(),
        key: if key.trim().is_empty() {
            None
        } else {
            Some(key.trim().to_owned())
        },
    });
    match save_book(&book) {
        Ok(()) => String::new(),
        Err(err) => err,
    }
}

/// 删掉一条，顺带丢掉它的指纹（下次连接重新走首次信任）。
pub fn book_remove(addr: &str) -> String {
    let mut book = match load_book() {
        Ok(book) => book,
        Err(err) => return err,
    };
    if !book.remove(addr) {
        return format!("error:{addr} is not in the address book");
    }
    match save_book(&book) {
        Ok(()) => String::new(),
        Err(err) => err,
    }
}

/// 把某个 `地址:端口` 标为已验证（操作者核过指纹之后）。
pub fn book_verify(addr: &str) -> String {
    let mut book = match load_book() {
        Ok(book) => book,
        Err(err) => return err,
    };
    if !book.mark_verified(addr) {
        return format!("error:{addr} has no pinned fingerprint yet");
    }
    match save_book(&book) {
        Ok(()) => String::new(),
        Err(err) => err,
    }
}

/// 可用的 client 身份（私钥）列表，JSON 数组，每项 `{name, path, public}`。
pub fn identity_list() -> String {
    #[derive(Serialize)]
    struct Identity {
        name: String,
        path: String,
        public: String,
    }
    let list: Vec<Identity> = crate::landesk::list_identities()
        .into_iter()
        .map(|path| {
            let public = hbb_common::keyauth::GlobalKeyPair::load(&path)
                .map(|kp| crate::landesk::public_key_hex(&kp.pk))
                .unwrap_or_default();
            Identity {
                name: path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().to_string())
                    .unwrap_or_default(),
                path: path.to_string_lossy().to_string(),
                public,
            }
        })
        .collect();
    serde_json::to_string(&list).unwrap_or_else(|err| format!("error:{err}"))
}

/// 服务端监听状态（`landesk-status.json`）的 JSON；没有该文件时返回 `null`。
pub fn listen_status() -> String {
    match crate::landesk::read_listen_status() {
        Some(status) => serde_json::to_string(&status).unwrap_or_else(|err| format!("error:{err}")),
        None => "null".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_without_a_port_is_refused() {
        // 不碰磁盘：校验发生在读写之前。
        assert!(book_upsert("g", "s", "n", "10.0.0.5", "").starts_with("error:"));
    }

    #[test]
    fn identity_list_is_json_even_when_empty() {
        let text = identity_list();
        assert!(text == "[]" || text.starts_with("[{"), "unexpected: {text}");
    }
}
