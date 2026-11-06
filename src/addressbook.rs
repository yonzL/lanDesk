//! Master 侧地址簿：组 / 子组两级 + 条目（显示名、`地址:端口`），以及每台机器钉住的主机公钥。
//!
//! 文件放在可执行文件同目录（`addressbook.json`），导入导出就是整份文件的进出——最终由图形界面
//! 接管，这里的 CLI 只是把格式和语义先定下来。设计见 `architecture.md`。

use std::path::{Path, PathBuf};

use hbb_common::bail;
use hbb_common::keyauth::{PinOutcome, PinnedHosts};
use hbb_common::sodiumoxide::crypto::sign;
use hbb_common::ResultType;
use serde_derive::{Deserialize, Serialize};
/// 一条地址簿记录。`group` / `subgroup` 只影响怎么找到机器，不承载任何权限含义。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub subgroup: String,
    #[serde(default)]
    pub name: String,
    /// `主机:端口`，与指纹库的键一致。
    pub addr: String,
    /// 连这台机器时用哪把 client 私钥（身份名或路径）；`None` 表示用默认身份。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AddressBook {
    #[serde(default)]
    entries: Vec<Entry>,
    /// 指纹库与条目一起存，避免"清单"和"钉住的公钥"两份数据各说各话。
    #[serde(default)]
    pins: PinnedHosts,
}

impl AddressBook {
    /// 可执行文件同目录下的默认文件。
    pub fn default_path() -> PathBuf {
        crate::landesk::exe_dir().join("addressbook.json")
    }

    pub fn load(path: &Path) -> ResultType<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let book: Self = serde_json::from_str(&text)
                    .map_err(|err| hbb_common::anyhow::anyhow!("{}: {err}", path.display()))?;
                Ok(book)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err).map_err(|err| {
                hbb_common::anyhow::anyhow!("reading {}: {err}", path.display())
            }),
        }
    }

    pub fn save(&self, path: &Path) -> ResultType<()> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir).ok();
            }
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|err| hbb_common::anyhow::anyhow!("serializing address book: {err}"))?;
        std::fs::write(path, text)
            .map_err(|err| hbb_common::anyhow::anyhow!("writing {}: {err}", path.display()))
    }

    /// 导入：读来源、校验格式，再写到目标位置。返回落盘后的地址簿。
    pub fn import(from: &Path, to: &Path) -> ResultType<Self> {
        let book = Self::load(from)?;
        if book.entries.is_empty() && book.pins.is_empty() {
            bail!("{} 里没有任何条目，未导入", from.display());
        }
        book.save(to)?;
        Ok(book)
    }

    /// 导出：把当前地址簿写到目标文件。
    pub fn export(&self, to: &Path) -> ResultType<()> {
        self.save(to)
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn get(&self, addr: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.addr == addr)
    }

    /// 按 `地址:端口` 覆盖式写入：同一地址只留一条。
    pub fn upsert(&mut self, entry: Entry) {
        match self.entries.iter_mut().find(|e| e.addr == entry.addr) {
            Some(existing) => *existing = entry,
            None => self.entries.push(entry),
        }
    }

    /// 删掉一个条目，顺带丢掉它的指纹，下次连接重新走首次信任。
    pub fn remove(&mut self, addr: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.addr != addr);
        self.pins.forget(addr);
        self.entries.len() != before
    }

    /// 指纹库：首次连接自动记录、指纹变化即拒的逻辑都在这里。
    pub fn pin_or_check(&mut self, addr: &str, host_pk: &sign::PublicKey) -> PinOutcome {
        self.pins.pin_or_check(addr, host_pk)
    }

    pub fn pins(&self) -> &PinnedHosts {
        &self.pins
    }

    pub fn pins_mut(&mut self) -> &mut PinnedHosts {
        &mut self.pins
    }

    pub fn mark_verified(&mut self, addr: &str) -> bool {
        self.pins.mark_verified(addr)
    }

    /// 忘掉某个地址的指纹记录（删条目时用）。
    pub fn forget(&mut self, addr: &str) -> bool {
        self.pins.forget(addr)
    }

    pub fn fingerprint_of(&self, addr: &str) -> Option<&str> {
        self.pins.get(addr).map(|host| host.fingerprint.as_str())
    }

    /// 按条目列表合并：`地址:端口` 已存在就覆盖，否则追加。返回新增/覆盖的条数。
    pub fn merge_entries(&mut self, entries: impl IntoIterator<Item = Entry>) -> usize {
        let mut count = 0;
        for mut entry in entries {
            // CSV 里通常不写身份列：不要把已有的身份信息覆盖成“未指定”。
            if entry.key.is_none() {
                if let Some(existing) = self.get(&entry.addr) {
                    entry.key = existing.key.clone();
                }
            }
            self.upsert(entry);
            count += 1;
        }
        count
    }

    /// 导出为 CSV：首行是表头，之后每行一个条目。
    pub fn to_csv(&self) -> String {
        let mut out = String::from("组,子组,显示名,地址,身份\n");
        for entry in &self.entries {
            out.push_str(&format!(
                "{},{},{},{},{}\n",
                csv_field(&entry.group),
                csv_field(&entry.subgroup),
                csv_field(&entry.name),
                csv_field(&entry.addr),
                csv_field(entry.key.as_deref().unwrap_or("")),
            ));
        }
        out
    }

    /// 按 CSV 文本合并条目：先解析全部行，全部合法才落地（不会导到一半留下半本书）。
    pub fn merge_csv(&mut self, text: &str) -> ResultType<usize> {
        let entries = parse_csv_entries(text)?;
        Ok(self.merge_entries(entries))
    }
}

fn csv_field(value: &str) -> String {
    if value.contains(['\"', ',']) {
        format!("\"{}\"", value.replace('\"', "\"\""))
    } else {
        value.to_owned()
    }
}

/// 最小 CSV 解析：支持引号包裹与双引号转义，不支持字段内换行（遇到就报错，而不是猜）。
fn parse_csv_rows(text: &str) -> ResultType<Vec<Vec<String>>> {
    let mut rows = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let mut fields = Vec::new();
        let mut field = String::new();
        let mut chars = line.chars().peekable();
        let mut in_quotes = false;
        while let Some(ch) = chars.next() {
            match ch {
                '"' if in_quotes => {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        field.push('"');
                    } else {
                        in_quotes = false;
                    }
                }
                '"' => in_quotes = true,
                ',' if !in_quotes => {
                    fields.push(std::mem::take(&mut field));
                }
                other => field.push(other),
            }
        }
        if in_quotes {
            bail!("第 {} 行的引号没有闭合", index + 1);
        }
        fields.push(field);
        rows.push(fields);
    }
    Ok(rows)
}

fn parse_csv_entries(text: &str) -> ResultType<Vec<Entry>> {
    let mut rows = parse_csv_rows(text)?;
    if rows.is_empty() {
        bail!("CSV 里没有任何条目");
    }
    let header: Vec<String> = rows.remove(0).iter().map(|h| h.trim().to_lowercase()).collect();
    let column = |names: &[&str]| -> Option<usize> {
        header
            .iter()
            .position(|h| names.iter().any(|name| name == h))
    };
    let Some(addr_column) = column(&["地址", "addr", "address", "host", "地址:端口"]) else {
        bail!("CSV 表头里找不到地址列（可用：地址 / addr / address / host）");
    };
    let group_column = column(&["组", "group"]);
    let subgroup_column = column(&["子组", "subgroup", "sub"]);
    let key_column = column(&["身份", "key", "identity"]);
    let name_column = column(&["显示名", "name", "display"]);
    let pick = |row: &[String], index: Option<usize>| -> String {
        index
            .and_then(|index| row.get(index))
            .map(|value| value.trim().to_owned())
            .unwrap_or_default()
    };
    let mut entries = Vec::new();
    for (offset, row) in rows.iter().enumerate() {
        let addr = pick(row, Some(addr_column));
        if addr.is_empty() {
            bail!("第 {} 行没有地址", offset + 2);
        }
        if !addr.contains(':') {
            bail!(
                "第 {} 行的地址缺少端口：{addr}（应用「地址:端口」）",
                offset + 2
            );
        }
        let key = pick(row, key_column);
        entries.push(Entry {
            group: pick(row, group_column),
            subgroup: pick(row, subgroup_column),
            name: pick(row, name_column),
            addr,
            key: if key.is_empty() { None } else { Some(key) },
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("landesk-addressbook-{tag}"));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    fn entry(addr: &str, name: &str) -> Entry {
        Entry {
            group: "办公室".into(),
            subgroup: "开发组".into(),
            name: name.into(),
            addr: addr.into(),
            key: None,
        }
    }

    #[test]
    fn upsert_replaces_the_same_address() {
        let mut book = AddressBook::default();
        book.upsert(entry("10.0.0.5:21118", "old"));
        book.upsert(entry("10.0.0.5:21118", "new"));
        book.upsert(entry("10.0.0.6:21118", "other"));
        assert_eq!(book.entries().len(), 2);
        assert_eq!(book.get("10.0.0.5:21118").expect("entry").name, "new");
    }

    #[test]
    fn roundtrips_entries_and_pins() {
        let _ = hbb_common::sodiumoxide::init();
        let dir = temp_dir("roundtrip");
        let path = dir.join("addressbook.json");
        let (pk, _) = sign::gen_keypair();
        let mut book = AddressBook::default();
        book.upsert(entry("10.0.0.5:21118", "工位"));
        assert_eq!(book.pin_or_check("10.0.0.5:21118", &pk), PinOutcome::New);
        book.save(&path).expect("save");

        let mut loaded = AddressBook::load(&path).expect("load");
        assert_eq!(loaded.entries(), book.entries());
        assert_eq!(
            loaded.fingerprint_of("10.0.0.5:21118"),
            book.fingerprint_of("10.0.0.5:21118")
        );
        assert_eq!(loaded.pin_or_check("10.0.0.5:21118", &pk), PinOutcome::Verified);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn import_writes_the_target_and_export_reads_it_back() {
        let _ = hbb_common::sodiumoxide::init();
        let dir = temp_dir("import");
        let source = dir.join("incoming.json");
        let target = dir.join("addressbook.json");
        let mut book = AddressBook::default();
        book.upsert(entry("10.0.0.7:21118", "机架机"));
        book.save(&source).expect("save source");

        let imported = AddressBook::import(&source, &target).expect("import");
        assert_eq!(imported.get("10.0.0.7:21118").expect("entry").name, "机架机");
        assert!(target.exists());

        let exported = dir.join("exported.json");
        imported.export(&exported).expect("export");
        let reloaded = AddressBook::load(&exported).expect("load exported");
        assert_eq!(reloaded.entries(), imported.entries());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn csv_roundtrips_entries() {
        let mut book = AddressBook::default();
        book.upsert(Entry {
            group: "办公室".into(),
            subgroup: "开发组".into(),
            name: "工位-01".into(),
            addr: "10.0.0.5:21118".into(),
            key: None,
        });
        let csv = book.to_csv();
        let mut imported = AddressBook::default();
        assert_eq!(imported.merge_csv(&csv).expect("merge"), 1);
        assert_eq!(imported.entries(), book.entries());
    }

    #[test]
    fn csv_handles_quoted_fields_and_english_header() {
        let text = "group,subgroup,name,address\n\"机房, A\",机架,\"机器 \"\"甲\"\"\",10.0.0.9:21118\n";
        let mut book = AddressBook::default();
        assert_eq!(book.merge_csv(text).expect("merge"), 1);
        let entry = book.get("10.0.0.9:21118").expect("entry");
        assert_eq!(entry.group, "机房, A");
        assert_eq!(entry.name, "机器 \"甲\"");
    }

    #[test]
    fn csv_replaces_the_same_address() {
        let mut book = AddressBook::default();
        book.upsert(entry("10.0.0.5:21118", "old"));
        let text = "组,子组,显示名,地址\n办公室,开发组,new,10.0.0.5:21118\n";
        book.merge_csv(text).expect("merge");
        assert_eq!(book.entries().len(), 1);
        assert_eq!(book.get("10.0.0.5:21118").expect("entry").name, "new");
    }

    #[test]
    fn csv_without_address_column_is_refused() {
        let mut book = AddressBook::default();
        let err = book.merge_csv("组,子组,显示名\n办公室,开发组,工位\n").unwrap_err();
        assert!(err.to_string().contains("地址列"));
    }

    #[test]
    fn csv_address_without_port_is_refused() {
        let mut book = AddressBook::default();
        let err = book
            .merge_csv("组,子组,显示名,地址\n办公室,开发组,工位,10.0.0.5\n")
            .unwrap_err();
        assert!(err.to_string().contains("端口"), "{err}");
    }

    #[test]
    fn csv_unbalanced_quote_is_refused() {
        let mut book = AddressBook::default();
        let err = book
            .merge_csv("组,子组,显示名,地址\n\"办公室,开发组,工位,10.0.0.5:21118\n")
            .unwrap_err();
        assert!(err.to_string().contains("引号"), "{err}");
    }

    #[test]
    fn removing_an_entry_also_drops_its_fingerprint() {
        let _ = hbb_common::sodiumoxide::init();
        let (pk, _) = sign::gen_keypair();
        let mut book = AddressBook::default();
        book.upsert(entry("10.0.0.5:21118", "工位"));
        book.pin_or_check("10.0.0.5:21118", &pk);
        assert!(book.fingerprint_of("10.0.0.5:21118").is_some());

        assert!(book.remove("10.0.0.5:21118"));
        assert!(book.get("10.0.0.5:21118").is_none());
        assert!(
            book.fingerprint_of("10.0.0.5:21118").is_none(),
            "删除条目后指纹也要丢，下次连接重新走首次信任"
        );
        assert!(!book.remove("10.0.0.5:21118"), "重复删除应返回 false");
    }

    #[test]
    fn empty_import_is_refused() {
        let dir = temp_dir("empty");
        let source = dir.join("empty.json");
        let target = dir.join("addressbook.json");
        std::fs::write(&source, "{\"entries\":[],\"pins\":{\"hosts\":{}}}").expect("write");
        assert!(AddressBook::import(&source, &target).is_err());
        std::fs::remove_dir_all(dir).ok();
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    fn entry(addr: &str, key: Option<&str>) -> Entry {
        Entry {
            group: "g".into(),
            subgroup: "s".into(),
            name: "n".into(),
            addr: addr.into(),
            key: key.map(|k| k.to_owned()),
        }
    }

    #[test]
    fn csv_keeps_the_identity_column() {
        let mut book = AddressBook::default();
        book.upsert(entry("10.0.0.5:21118", Some("master-a")));
        let csv = book.to_csv();
        let mut restored = AddressBook::default();
        restored.merge_csv(&csv).expect("merge");
        assert_eq!(
            restored.get("10.0.0.5:21118").and_then(|e| e.key.clone()),
            Some("master-a".to_owned())
        );
    }

    #[test]
    fn a_csv_without_the_identity_column_keeps_the_existing_one() {
        let mut book = AddressBook::default();
        book.upsert(entry("10.0.0.5:21118", Some("master-a")));
        // 老格式：没有身份列
        book.merge_csv("组,子组,显示名,地址\ng,s,n2,10.0.0.5:21118\n")
            .expect("merge");
        let entry = book.get("10.0.0.5:21118").expect("entry");
        assert_eq!(entry.name, "n2");
        assert_eq!(entry.key.as_deref(), Some("master-a"));
    }
}
