//! lanDesk 的最小 Master 窗口：左侧组/子组树，右侧选中机器的信息与连接按钮，底部一行日志。
//!
//! 它只做三件事：读地址簿、拨号（复用 `librustdesk::transport::dial`）、把人能看懂的结果写在底部。
//! 设计取舍见 `architecture.md`：先要一个能在这台机器上编译、能真的连上去的最小界面。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use eframe::egui;
use hbb_common::keyauth::PinOutcome;
use librustdesk::addressbook::{AddressBook, Entry};
use librustdesk::landesk::exe_dir;

fn main() -> eframe::Result<()> {
    let dir = exe_dir();
    let app = App::new(dir.join("addressbook.json"), dir.join("private.key"));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([820.0, 560.0])
            .with_title("lanDesk"),
        ..Default::default()
    };
    eframe::run_native("lanDesk", options, Box::new(|_cc| Ok(Box::new(app))))
}

struct App {
    book_path: PathBuf,
    key_path: PathBuf,
    book: AddressBook,
    load_error: Option<String>,
    selected: Option<String>,
    log: Vec<String>,
    receiver: Option<Receiver<Result<PinOutcome, String>>>,
}

impl App {
    fn new(book_path: PathBuf, key_path: PathBuf) -> Self {
        let mut app = Self {
            book_path,
            key_path,
            book: AddressBook::default(),
            load_error: None,
            selected: None,
            log: Vec::new(),
            receiver: None,
        };
        app.reload();
        app
    }

    fn reload(&mut self) {
        match AddressBook::load(&self.book_path) {
            Ok(book) => {
                self.book = book;
                self.load_error = None;
            }
            Err(err) => self.load_error = Some(format!("{err:#}")),
        }
        if let Some(selected) = &self.selected {
            if self.book.get(selected).is_none() {
                self.selected = None;
            }
        }
    }

    /// 分组：组 → 子组 → 条目。只影响"怎么找到机器"，不承载权限。
    fn tree(&self) -> BTreeMap<String, BTreeMap<String, Vec<Entry>>> {
        let mut tree: BTreeMap<String, BTreeMap<String, Vec<Entry>>> = BTreeMap::new();
        for entry in self.book.entries() {
            tree.entry(entry.group.clone())
                .or_default()
                .entry(entry.subgroup.clone())
                .or_default()
                .push(entry.clone());
        }
        tree
    }

    fn connect(&mut self, addr: String, control: bool) {
        if self.receiver.is_some() {
            return;
        }
        self.log.push(if control { format!("连接 {addr}（申请操作权）…") } else { format!("连接 {addr}（只看）…") });
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        let book_path = self.book_path.clone();
        let key_path = self.key_path.clone();
        std::thread::spawn(move || {
            let result = match hbb_common::tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime
                    .block_on(librustdesk::transport::dial(&addr, &book_path, &key_path, control))
                    .map_err(|err| format!("{err:#}")),
                Err(err) => Err(format!("{err}")),
            };
            tx.send(result).ok();
        });
    }

    fn mark_verified(&mut self, addr: &str) {
        if self.book.mark_verified(addr) {
            match self.book.save(&self.book_path) {
                Ok(()) => self.log.push(format!(
                    "{addr} 标为已验证（{}）",
                    self.book.fingerprint_of(addr).unwrap_or("?")
                )),
                Err(err) => self.log.push(format!("保存地址簿失败：{err:#}")),
            }
            self.reload();
        }
    }

    fn pin_state(&self, addr: &str) -> (String, bool) {
        match self.book.pins().get(addr) {
            None => ("未连接过".to_owned(), false),
            Some(host) if host.verified => (format!("已验证 {}", host.fingerprint), false),
            Some(host) => (format!("未验证 {}", host.fingerprint), true),
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.receiver {
            match rx.try_recv() {
                Ok(result) => {
                    self.receiver = None;
                    match result {
                        Ok(PinOutcome::New) => {
                            self.log.push("连接成功；这台机器是首次连接，指纹已记录（未验证）".into())
                        }
                        Ok(PinOutcome::Verified) => self.log.push("连接成功；指纹与地址簿一致".into()),
                        Ok(PinOutcome::Changed) => self.log.push("连接被拒：指纹变了".into()),
                        Err(err) => self.log.push(format!("连接失败：{err}")),
                    }
                    self.reload();
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(200))
                }
                Err(mpsc::TryRecvError::Disconnected) => self.receiver = None,
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("lanDesk");
                ui.separator();
                if ui.button("重新读取地址簿").clicked() {
                    self.reload();
                }
                ui.label(format!("地址簿：{}", self.book_path.display()));
            });
            if let Some(err) = &self.load_error {
                ui.colored_label(egui::Color32::RED, format!("地址簿读取失败：{err}"));
            }
            ui.add_space(4.0);
        });

        egui::SidePanel::left("tree")
            .resizable(true)
            .default_width(300.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                if self.book.entries().is_empty() {
                    ui.label("地址簿是空的。");
                    ui.label("用命令行导入一份清单：");
                    ui.monospace("rustdesk.exe --landesk-book-import hosts.csv");
                    return;
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let tree = self.tree();
                    for (group, subgroups) in tree {
                        let group_name = if group.is_empty() {
                            "（未分组）".to_owned()
                        } else {
                            group.clone()
                        };
                        egui::CollapsingHeader::new(group_name)
                            .default_open(true)
                            .show(ui, |ui| {
                                for (subgroup, entries) in subgroups {
                                    let subgroup_name = if subgroup.is_empty() {
                                        "（未分子组）".to_owned()
                                    } else {
                                        subgroup.clone()
                                    };
                                    egui::CollapsingHeader::new(subgroup_name)
                                        .default_open(true)
                                        .show(ui, |ui| {
                                            for entry in entries {
                                                let label = if entry.name.is_empty() {
                                                    entry.addr.clone()
                                                } else {
                                                    format!("{}  ({})", entry.name, entry.addr)
                                                };
                                                let selected =
                                                    self.selected.as_deref() == Some(&entry.addr);
                                                if ui.selectable_label(selected, label).clicked() {
                                                    self.selected = Some(entry.addr.clone());
                                                }
                                            }
                                        });
                                }
                            });
                    }
                });
            });

        egui::TopBottomPanel::bottom("log").show(ctx, |ui| {
            ui.add_space(2.0);
            egui::ScrollArea::vertical()
                .max_height(120.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in &self.log {
                        ui.label(line);
                    }
                });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let Some(addr) = self.selected.clone() else {
                ui.label("从左边的地址簿里选一台机器。");
                return;
            };
            let Some(entry) = self.book.get(&addr).cloned() else {
                return;
            };
            ui.add_space(6.0);
            ui.heading(if entry.name.is_empty() {
                addr.clone()
            } else {
                entry.name.clone()
            });
            ui.label(format!("地址：{}", entry.addr));
            let (state, can_verify) = self.pin_state(&addr);
            ui.label(format!("主机指纹：{state}"));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let busy = self.receiver.is_some();
                if ui
                    .add_enabled(!busy, egui::Button::new("连接（申请操作权）"))
                    .on_hover_text("验证对面主机身份、用本机私钥证明自己，并拿操作权（会把当前操作者降为只读）")
                    .clicked()
                {
                    self.connect(addr.clone(), true);
                }
                if ui
                    .add_enabled(!busy, egui::Button::new("只看"))
                    .on_hover_text("同样验证身份与私钥，但不抢操作权：只收画面")
                    .clicked()
                {
                    self.connect(addr.clone(), false);
                }
                if can_verify && ui.button("核过指纹，标为已验证").clicked() {
                    self.mark_verified(&addr);
                }
            });
            ui.add_space(12.0);
            ui.separator();
            ui.label(format!("私钥：{}", self.key_path.display()));
            ui.small("私钥与地址簿都在本程序同目录；换位置用命令行参数 --landesk-key / --landesk-book。");
        });
    }
}
