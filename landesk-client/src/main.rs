//! lanDesk 的最小 Master 窗口：左侧组/子组树，右侧选中机器的信息；连上之后把远端画面画在窗口里，
//! 并把键鼠转成上游的输入消息。
//!
//! 设计取舍见 `architecture.md`：会话本体（登录、解码、输入编码）用上游的 `Remote`，这里只做接线
//! 与画面呈现。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use base::message_proto::{key_event, ControlKey, KeyEvent, KeyboardMode};
use eframe::egui;

use librustdesk::addressbook::{AddressBook, Entry};
use librustdesk::common::input::{
    MOUSE_BUTTON_LEFT, MOUSE_BUTTON_RIGHT, MOUSE_TYPE_DOWN, MOUSE_TYPE_MOVE, MOUSE_TYPE_UP,
    MOUSE_TYPE_WHEEL,
};
use librustdesk::landesk::exe_dir;
use librustdesk::landesk::session::SessionHandle;

fn main() -> eframe::Result<()> {
    let dir = exe_dir();
    let app = App::new(dir.join("addressbook.json"), dir.join("private.key"));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 680.0])
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
    session: Option<SessionHandle>,
    session_peer: Option<String>,
    texture: Option<egui::TextureHandle>,
    last_frame_seq: u64,
    remote_size: (usize, usize),
    file_clipboard_requested: bool,
    last_pointer: Option<(i32, i32)>,
    last_modifiers: egui::Modifiers,
    current_display: i32,
    /// 文件分发的标签页（false = 远端画面）。
    show_distribute: bool,
    /// 分发目标（地址簿里的 `地址:端口`）。
    targets: std::collections::BTreeSet<String>,
    /// 远端目标目录。
    distribute_dir: String,
    /// 要推的文件，一行一个本机路径。
    distribute_files: String,
    distribution: Option<librustdesk::landesk::session::Distribution>,
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
            session: None,
            session_peer: None,
            texture: None,
            last_frame_seq: 0,
            remote_size: (0, 0),
            file_clipboard_requested: false,
            last_pointer: None,
            last_modifiers: egui::Modifiers::NONE,
            current_display: 0,
            show_distribute: false,
            targets: Default::default(),
            distribute_dir: "C:\\landesk-incoming".to_owned(),
            distribute_files: String::new(),
            distribution: None,
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
        if self.session.is_some() {
            return;
        }
        self.log.push(if control {
            format!("连接 {addr}（申请操作权）…")
        } else {
            format!("连接 {addr}（只看）…")
        });
        match librustdesk::landesk::session::start(
            addr.clone(),
            self.book_path.clone(),
            self.key_path.clone(),
            control,
        ) {
            Ok(session) => {
                self.session = Some(session);
                self.session_peer = Some(addr.clone());
                self.selected = Some(addr);
            }
            Err(err) => self.log.push(format!("连接失败：{err:#}")),
        }
    }

    fn disconnect(&mut self) {
        if let Some(session) = self.session.take() {
            session.close();
            self.log.push("已断开".into());
        }
        self.session_peer = None;
        self.texture = None;
        self.last_frame_seq = 0;
        self.remote_size = (0, 0);
        self.reload();
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

    /// 把新帧传成纹理。只在序号变化时做——一帧 1080p 约 8MB。
    fn pump_frames(&mut self, ctx: &egui::Context) {
        let Some(session) = &self.session else {
            return;
        };
        // 会话期间的提示（已连接 / 登录失败 / 对端消息）都显示在底部日志里。
        for message in session.drain_messages() {
            self.log.push(message);
        }
        let seq = session.frame_seq();
        if session.is_closed() {
            // 先排干剩下的提示，再回到可重连的状态。
            for message in session.drain_messages() {
                self.log.push(message);
            }
            self.log.push("会话已结束".into());
            self.session = None;
            self.session_peer = None;
            self.texture = None;
            self.last_frame_seq = 0;
            self.remote_size = (0, 0);
            self.file_clipboard_requested = false;
            self.reload();
            return;
        }
        if seq == self.last_frame_seq {
            ctx.request_repaint_after(Duration::from_millis(16));
            return;
        }
        let Some(frame) = session.latest_frame() else {
            return;
        };
        if frame.width == 0 || frame.height == 0 || frame.rgba.len() < frame.width * frame.height * 4 {
            return;
        }
        self.last_frame_seq = frame.seq;
        self.remote_size = (frame.width, frame.height);
        if !self.file_clipboard_requested {
            // 会话真正立起来之后再把剪贴板传文件打开（开关是对端侧生效的选项）。
            self.file_clipboard_requested = true;
            session.set_file_clipboard(true);
            self.log.push("已开启剪贴板传文件（在资源管理器复制文件，到远端粘贴）".into());
        }
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [frame.width, frame.height],
            &frame.rgba[..frame.width * frame.height * 4],
        );
        match &mut self.texture {
            Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
            None => {                self.texture =
                    Some(ctx.load_texture("landesk-remote", image, egui::TextureOptions::LINEAR));
                self.log.push(format!(
                    "收到画面 {}x{}",
                    self.remote_size.0, self.remote_size.1
                ));
            }
        }
    }

    /// 文件分发：多选目标 + 填文件 + 逐台进度。
    fn draw_distribute(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.heading("把同一批文件分发给多台机器");
        ui.label("先在左侧勾选目标（可多选），再填远端目录与要推的文件。");
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            ui.label("远端目录：");
            ui.add(
                egui::TextEdit::singleline(&mut self.distribute_dir)
                    .desired_width(420.0)
                    .hint_text("例如 C:\\landesk-incoming"),
            );
            ui.label(format!("目标 {} 台", self.targets.len()));
        });
        ui.add_space(4.0);
        ui.label("要推的文件（一行一个本机路径）：");
        ui.add(
            egui::TextEdit::multiline(&mut self.distribute_files)
                .desired_rows(4)
                .desired_width(f32::INFINITY)
                .hint_text("C:\\build\\agent.exe\nC:\\build\\config.json"),
        );
        ui.add_space(6.0);

        let running = self
            .distribution
            .as_ref()
            .map(|d| !d.finished())
            .unwrap_or(false);
        let files: Vec<String> = self
            .distribute_files
            .lines()
            .map(|line| line.trim().to_owned())
            .filter(|line| !line.is_empty())
            .collect();
        let can_start = !running && !self.targets.is_empty() && !files.is_empty();
        if ui
            .add_enabled(can_start, egui::Button::new("开始分发"))
            .clicked()
        {
            let peers: Vec<String> = self.targets.iter().cloned().collect();
            self.log.push(format!(
                "分发 {} 个文件到 {} 台机器…",
                files.len(),
                peers.len()
            ));
            self.distribution = Some(librustdesk::landesk::session::distribute(
                peers,
                self.book_path.clone(),
                self.key_path.clone(),
                files,
                self.distribute_dir.clone(),
            ));
        }
        if running {
            ui.label("进行中…");
        }

        ui.add_space(10.0);
        ui.separator();
        match &self.distribution {
            Some(distribution) => {
                for report in distribution.snapshot() {
                    let state = if report.failed > 0 {
                        format!(
                            "失败 {} / 共 {}：{}",
                            report.failed,
                            report.total,
                            report.last_error.unwrap_or_default()
                        )
                    } else {
                        format!("{}/{} 完成", report.done, report.total)
                    };
                    ui.horizontal(|ui| {
                        ui.monospace(&report.peer);
                        ui.label(state);
                    });
                }
            }
            None => {
                ui.label("还没有开始过分发。");
            }
        }
    }

    fn draw_remote(&mut self, ui: &mut egui::Ui) {
        let Some(texture) = self.texture.clone() else {
            ui.label("已连上，正在等画面…");
            return;
        };
        let (rw, rh) = (self.remote_size.0 as f32, self.remote_size.1 as f32);
        let available = ui.available_size();
        let scale = (available.x / rw).min(available.y / rh).min(1.0);
        let size = egui::vec2(rw * scale, rh * scale);
        let (rect, response) =
            ui.allocate_exact_size(size, egui::Sense::click_and_drag());
        ui.painter().image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );

        let Some(session) = &self.session else {
            return;
        };
        let (modifiers, scroll, events, pressed, released) = ui.input(|i| {
            (
                i.modifiers,
                i.raw_scroll_delta.y,
                i.events.clone(),
                i.pointer.button_pressed(egui::PointerButton::Primary),
                i.pointer.button_released(egui::PointerButton::Primary),
            )
        });
        let (alt, ctrl, shift, command) = (
            modifiers.alt,
            modifiers.ctrl,
            modifiers.shift,
            modifiers.command,
        );
        // 修饰键：Ctrl/Shift/Alt/Meta 的状态变化要真的发过去，否则 Ctrl+C 这类组合在远端无效。
        for (now, before, key) in [
            (modifiers.ctrl, self.last_modifiers.ctrl, ControlKey::Control),
            (modifiers.shift, self.last_modifiers.shift, ControlKey::Shift),
            (modifiers.alt, self.last_modifiers.alt, ControlKey::Alt),
            (modifiers.command, self.last_modifiers.command, ControlKey::Meta),
        ] {
            if now != before {
                let mut evt = KeyEvent::new();
                evt.set_control_key(key);
                evt.down = now;
                evt.press = now;
                session.send_key(&evt);
            }
        }
        self.last_modifiers = modifiers;

        let to_remote = |pos: egui::Pos2| -> (i32, i32) {
            let x = ((pos.x - rect.min.x) / rect.width().max(1.0) * rw).clamp(0.0, rw - 1.0);
            let y = ((pos.y - rect.min.y) / rect.height().max(1.0) * rh).clamp(0.0, rh - 1.0);
            (x as i32, y as i32)
        };
        if let Some(pos) = response.hover_pos() {
            let (x, y) = to_remote(pos);
            if self.last_pointer != Some((x, y)) {
                self.last_pointer = Some((x, y));
                session.send_mouse(MOUSE_TYPE_MOVE, x, y, alt, ctrl, shift, command);
            }
        }
        if let Some(pos) = response.interact_pointer_pos() {
            let (x, y) = to_remote(pos);
            if pressed {
                session.send_mouse(
                    MOUSE_TYPE_DOWN | MOUSE_BUTTON_LEFT,
                    x,
                    y,
                    alt,
                    ctrl,
                    shift,
                    command,
                );
            }
            if released {
                session.send_mouse(
                    MOUSE_TYPE_UP | MOUSE_BUTTON_LEFT,
                    x,
                    y,
                    alt,
                    ctrl,
                    shift,
                    command,
                );
            }
        }
        if let Some(pos) = response.hover_pos() {
            if scroll.abs() > 0.5 {
                let (x, _y) = to_remote(pos);
                session.send_mouse(
                    MOUSE_TYPE_WHEEL,
                    x,
                    (scroll * -10.0) as i32,
                    alt,
                    ctrl,
                    shift,
                    command,
                );
            }
        }
        for event in events {
            match event {
                // 可打印字符走 legacy 模式的 unicode 通道：目标机自己按自己的键盘布局查键码。
                egui::Event::Text(text) => {
                    for ch in text.chars() {
                        let mut evt = KeyEvent::new();
                        evt.set_chr(ch as u32);
                        evt.down = true;
                        evt.press = true;
                        evt.mode = KeyboardMode::Legacy.into();
                        session.send_key(&evt);
                    }
                }
                egui::Event::Key {
                    key,
                    pressed,
                    repeat: _,
                    modifiers: _,
                    physical_key: _,
                } => {
                    let Some(control_key) = control_key_of(key) else {
                        continue;
                    };
                    let mut evt = KeyEvent::new();
                    evt.set_control_key(control_key);
                    evt.down = pressed;
                    evt.press = pressed;
                    session.send_key(&evt);
                }
                _ => {}
            }
        }
        let _ = key_event::Union::Chr(0); // 保持对生成类型路径的引用，避免 cfg 差异导致未用告警
        let _ = MOUSE_BUTTON_RIGHT;
    }
}

fn control_key_of(key: egui::Key) -> Option<ControlKey> {
    Some(match key {
        egui::Key::Enter => ControlKey::Return,
        egui::Key::Backspace => ControlKey::Backspace,
        egui::Key::Tab => ControlKey::Tab,
        egui::Key::Escape => ControlKey::Escape,
        egui::Key::Delete => ControlKey::Delete,
        egui::Key::Insert => ControlKey::Insert,
        egui::Key::Home => ControlKey::Home,
        egui::Key::End => ControlKey::End,
        egui::Key::PageUp => ControlKey::PageUp,
        egui::Key::PageDown => ControlKey::PageDown,
        egui::Key::ArrowLeft => ControlKey::LeftArrow,
        egui::Key::ArrowRight => ControlKey::RightArrow,
        egui::Key::ArrowUp => ControlKey::UpArrow,
        egui::Key::ArrowDown => ControlKey::DownArrow,
        egui::Key::Space => ControlKey::Space,
        egui::Key::F1 => ControlKey::F1,
        egui::Key::F2 => ControlKey::F2,
        egui::Key::F3 => ControlKey::F3,
        egui::Key::F4 => ControlKey::F4,
        egui::Key::F5 => ControlKey::F5,
        egui::Key::F6 => ControlKey::F6,
        egui::Key::F7 => ControlKey::F7,
        egui::Key::F8 => ControlKey::F8,
        egui::Key::F9 => ControlKey::F9,
        egui::Key::F10 => ControlKey::F10,
        egui::Key::F11 => ControlKey::F11,
        egui::Key::F12 => ControlKey::F12,
        _ => return None,
    })
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pump_frames(ctx);

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("lanDesk");
                ui.separator();
                ui.selectable_value(&mut self.show_distribute, false, "远端画面");
                ui.selectable_value(&mut self.show_distribute, true, "文件分发");
                ui.separator();
                if ui.button("重新读取地址簿").clicked() {
                    self.reload();
                }
                if let Some(peer) = &self.session_peer {
                    ui.separator();
                    ui.label(format!("已连接：{peer}"));
                    if ui.button("断开").clicked() {
                        self.disconnect();
                    }
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
            .default_width(280.0)
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
                                                ui.horizontal(|ui| {
                                                    let mut is_target =
                                                        self.targets.contains(&entry.addr);
                                                    if ui
                                                        .checkbox(&mut is_target, "")
                                                        .on_hover_text("作为文件分发的目标")
                                                        .changed()
                                                    {
                                                        if is_target {
                                                            self.targets.insert(entry.addr.clone());
                                                        } else {
                                                            self.targets.remove(&entry.addr);
                                                        }
                                                    }
                                                    let selected = self.selected.as_deref()
                                                        == Some(&entry.addr);
                                                    if ui
                                                        .selectable_label(selected, label)
                                                        .clicked()
                                                    {
                                                        self.selected = Some(entry.addr.clone());
                                                    }
                                                });
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
                .max_height(110.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in &self.log {
                        ui.label(line);
                    }
                });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.show_distribute {
                self.draw_distribute(ui);
                return;
            }
            if let Some(peer) = self.session_peer.clone() {
                ui.horizontal(|ui| {
                    ui.label(format!("远端：{peer}"));
                    let (state, _) = self.pin_state(&peer);
                    ui.label(format!("主机指纹：{state}"));
                    // 对端有不止一块屏时给个切换；切换后等新屏的第一帧。
                    let displays = self
                        .session
                        .as_ref()
                        .map(|session| session.displays())
                        .unwrap_or_default();
                    if displays.len() > 1 {
                        ui.separator();
                        ui.label("屏幕：");
                        for (index, width, height) in displays {
                            let label = format!("#{} {}x{}", index + 1, width, height);
                            if ui
                                .selectable_label(self.current_display == index, label)
                                .clicked()
                                && self.current_display != index
                            {
                                self.current_display = index;
                                self.texture = None;
                                self.last_frame_seq = 0;
                                if let Some(session) = &self.session {
                                    session.switch_display(index);
                                }
                            }
                        }
                    }
                });
                ui.separator();
                self.draw_remote(ui);
                return;
            }

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
                if ui
                    .button("连接（申请操作权）")
                    .on_hover_text("验证对面主机、用本机私钥证明自己，然后进入会话")
                    .clicked()
                {
                    self.connect(addr.clone(), true);
                }
                if ui
                    .button("只看")
                    .on_hover_text("同样验证身份，但声明自己不发送输入")
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
