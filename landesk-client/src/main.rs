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
    // 库里的日志默认没有安装 logger 会被丢弃；装上后才能看到会话/剪贴板/编码器的实际行为。
    // 先把产品名设成 lanDesk：`init_log` 用它算日志目录，否则日志会落到 `RustDesk\log\`。
    *hbb_common::config::APP_NAME.write().unwrap() = "lanDesk".to_owned();
    hbb_common::init_log(false, "client");
    paste_watcher::spawn();
    let dir = exe_dir();
    // QQ 形态：主窗口是地址簿；每个远程会话是一个**独立进程的窗口**
    // （`--landesk-remote <addr:port>`），与上游 RustDesk 的 `--connect` 一个思路。
    let mut args = std::env::args().skip(1);
    let mut remote_addr = None;
    while let Some(arg) = args.next() {
        if arg == "--landesk-remote" {
            remote_addr = args.next();
        }
    }
    let title = match &remote_addr {
        Some(addr) => format!("lanDesk — {addr}"),
        None => "lanDesk".to_owned(),
    };
    let app = App::new(
        dir.join("addressbook.json"),
        dir.join("private.key"),
        remote_addr,
    );
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 680.0])
            .with_title(&title),
        ..Default::default()
    };
    eframe::run_native(
        &title,
        options,
        Box::new(|cc| {
            install_cjk_font(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
}

/// 界面文案是英文，但后端回的消息可能是中文；装一个系统自带的 CJK 字体，避免豆腐块。
fn install_cjk_font(ctx: &egui::Context) {
    // 优先**单文件 TTF**：egui 底层用 ab_glyph，对 `.ttc`（TrueType Collection）支持不可靠，
    // 之前列表以 msyh.ttc 开头、文件存在就 return，结果一个字形也没装上 → 界面全是豆腐块。
    const CANDIDATES: [&str; 9] = [
        "C:\\Windows\\Fonts\\Deng.ttf",
        "C:\\Windows\\Fonts\\simhei.ttf",
        "C:\\Windows\\Fonts\\simkai.ttf",
        "C:\\Windows\\Fonts\\simfang.ttf",
        "C:\\Windows\\Fonts\\SIMYOU.TTF",
        "C:\\Windows\\Fonts\\STKAITI.TTF",
        // 最后才退回字体集合（有些机器只有这些）
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\simsun.ttc",
        "C:\\Windows\\Fonts\\SimsunExtG.ttf",
    ];
    for path in CANDIDATES {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let mut fonts = egui::FontDefinitions::default();
        fonts
            .font_data
            .insert("cjk".to_owned(), egui::FontData::from_owned(bytes).into());
        // 再挂一个符号字体：`✓`/`→`/`·` 这类在中文字体里常常缺字（曾导致豆腐块）。
        if let Ok(symbols) = std::fs::read("C:\\Windows\\Fonts\\seguisym.ttf") {
            fonts
                .font_data
                .insert("symbols".to_owned(), egui::FontData::from_owned(symbols).into());
        }
        // 追加到两个家族的末尾：拉丁字母仍用 egui 自带字体，缺字时才回退到 CJK/符号字体。
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            let list = fonts.families.entry(family).or_default();
            list.push("cjk".to_owned());
            if fonts.font_data.contains_key("symbols") {
                list.push("symbols".to_owned());
            }
        }
        ctx.set_fonts(fonts);
        hbb_common::log::info!("landesk: cjk font loaded from {path}");
        return;
    }
    hbb_common::log::warn!("landesk: no cjk font found, non-latin text will show as boxes");
}

struct App {
    /// `Some(addr)` = 这个进程是一个独立的远程会话窗口（QQ 形态里的“聊天窗口”）；
    /// `None` = 主窗口（地址簿）。
    remote_addr: Option<String>,
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
    /// 正在编辑的条目（`None` 表示没在编辑）。
    editing: Option<Entry>,
    /// 当前画质与帧率（镜像会话里的取值，用于界面高亮）。
    quality: String,
    fps: i32,
    last_frame_at: Option<std::time::Instant>,
    /// 主窗口上一次重读地址簿的时间。
    last_book_poll: Option<std::time::Instant>,
    /// 上一次看到的窗口焦点状态；变化时才调整优先级与帧率。
    last_focused: Option<bool>,
}

impl App {
    fn new(book_path: PathBuf, key_path: PathBuf, remote_addr: Option<String>) -> Self {
        let mut app = Self {
            remote_addr,
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
            editing: None,
            quality: "low".to_owned(),
            fps: 30,
            last_frame_at: None,
            last_book_poll: None,
            last_focused: None,
        };
        app.reload();
        // 远程窗口模式：进程起来就直接连，不需要用户在子窗口里再点一次。
        if let Some(addr) = app.remote_addr.clone() {
            app.connect(addr, true);
        }
        app
    }

    /// 每 2 秒重读一次地址簿（文件很小）：会话是独立进程开的，
    /// 它写回的指纹/条目只有重读才会反映到主窗口。
    fn poll_book_reload(&mut self) {
        const INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
        let now = std::time::Instant::now();
        let due = self
            .last_book_poll
            .map(|last| now.duration_since(last) >= INTERVAL)
            .unwrap_or(true);
        if due {
            self.last_book_poll = Some(now);
            self.reload();
        }
    }

    fn reload(&mut self) {        match AddressBook::load(&self.book_path) {
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

    /// 这台机器用哪把私钥：条目指定 → 默认身份 → 启动时的 key 路径。
    fn key_for(&self, addr: &str) -> Option<PathBuf> {
        if let Some(name) = self.book.get(addr).and_then(|entry| entry.key.as_deref()) {
            if let Some(path) = librustdesk::landesk::resolve_identity(name) {
                return Some(path);
            }
        }
        if self.key_path.is_file() {
            return Some(self.key_path.clone());
        }
        librustdesk::landesk::default_identity()
    }

    fn connect(&mut self, addr: String, control: bool) {
        if self.session.is_some() {
            return;
        }
        let Some(key) = self.key_for(&addr) else {
            self.log.push("No client identity: generate one or import a key first".into());
            return;
        };
        self.log.push(if control {
            format!("Connecting to {addr} (take control)…")
        } else {
            format!("Connecting to {addr} (view only)…")
        });
        match librustdesk::landesk::session::start(
            addr.clone(),
            self.book_path.clone(),
            key,
            control,
        ) {
            Ok(session) => {
                paste_watcher::set(Some(session.clone()));
                self.session = Some(session);
                self.session_peer = Some(addr.clone());
                self.selected = Some(addr.clone());
            }
            Err(err) => self.log.push(format!("Connect failed: {err:#}")),
        }
    }

    fn disconnect(&mut self) {
        if let Some(session) = self.session.take() {
            session.close();
            self.log.push("Disconnected".into());
        }
        paste_watcher::set(None);
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
                    "{addr} marked verified ({})",
                    self.book.fingerprint_of(addr).unwrap_or("?")
                )),
                Err(err) => self.log.push(format!("Saving the address book failed: {err:#}")),
            }
            self.reload();
        }
    }

    /// 首次使用的两件事：有没有私钥、地址簿里有没有东西。
    fn draw_first_run_hints(&mut self, ui: &mut egui::Ui) {
        if !self.key_path.is_file() {
            ui.colored_label(
                egui::Color32::YELLOW,
                format!("No private key yet: {}", self.key_path.display()),
            );
            ui.small("Generate a key pair here, then copy public.key to every service machine.");
            if ui.button("Generate key pair").clicked() {
                // 生成到身份目录，文件名带主机/时间/IP，便于多把并存时区分。
                let dir = librustdesk::landesk::identities_dir();
                match std::fs::create_dir_all(&dir)
                    .map_err(|err| hbb_common::anyhow::anyhow!("creating {}: {err}", dir.display()))
                    .and_then(|_| librustdesk::landesk::generate_named_key_pair("master", &dir))
                {
                    Ok((private, public)) => self.log.push(format!(
                        "Generated {} — copy {} to the service machine(s)",
                        private.display(),
                        public.display()
                    )),
                    Err(err) => self.log.push(format!("{err:#}")),
                }
            }
            ui.add_space(6.0);
        }
    }

    fn pin_state(&self, addr: &str) -> (String, bool) {
        match self.book.pins().get(addr) {
            None => ("never connected".to_owned(), false),
            Some(host) if host.verified => (format!("verified {}", host.fingerprint), false),
            Some(host) => (format!("unverified {}", host.fingerprint), true),
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
            self.log.push("Session ended".into());
            paste_watcher::set(None);
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
            // 后台窗口不必要 60Hz 轮询：降到 5Hz，把 CPU 留给前台会话。
            let idle_ms = if self.last_focused == Some(false) { 200 } else { 16 };
            ctx.request_repaint_after(Duration::from_millis(idle_ms));
            return;
        }
        let Some(frame) = session.latest_frame() else {
            return;
        };
        if frame.width == 0 || frame.height == 0 || frame.rgba.len() < frame.width * frame.height * 4 {
            return;
        }
        // 每 30 帧记一次到达间隔：用来分辨“帧率低”与“网络成批到达”。
        if frame.seq % 30 == 0 {
            let now = std::time::Instant::now();
            let dt = self
                .last_frame_at
                .map(|last| now.duration_since(last).as_millis())
                .unwrap_or(0);
            self.last_frame_at = Some(now);
            hbb_common::log::info!(
                "frame {} {}x{} ({} ms since the previous sample)",
                frame.seq,
                frame.width,
                frame.height,
                dt
            );
        }
        self.last_frame_seq = frame.seq;
        self.remote_size = (frame.width, frame.height);
        if !self.file_clipboard_requested {
            // 会话真正立起来之后再把剪贴板传文件打开（开关是对端侧生效的选项）。
            self.file_clipboard_requested = true;
            session.set_file_clipboard(true);
            // 默认低画质：VM 里编码是瓶颈时，这个比“等它慢慢编”实际得多；界面里可随时改。
            session.set_image_quality(&self.quality);
            self.log.push(format!(
                "File clipboard enabled; quality {}",
                self.quality
            ));
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
                    "Got a frame {}x{}",
                    self.remote_size.0, self.remote_size.1
                ));
            }
        }
    }

    /// 文件分发：多选目标 + 填文件 + 逐台进度。
    fn draw_distribute(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.heading("Push the same files to several machines");
        ui.add_space(6.0);

        // 目标就在左侧地址簿里勾：这里只把结果回显出来，不再做一套重复的选择界面。
        if self.targets.is_empty() {
            ui.colored_label(
                egui::Color32::YELLOW,
                "Tick one or more machines in the address book on the left first.",
            );
        } else {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("Targets ({})：", self.targets.len()));
                let targets: Vec<String> = self.targets.iter().cloned().collect();
                for addr in targets {
                    if ui.small_button(format!("{addr}  x")).on_hover_text("Remove from this push").clicked() {
                        self.targets.remove(&addr);
                    }
                }
            });
        }

        ui.horizontal(|ui| {
            ui.label("Remote directory:");
            ui.add(
                egui::TextEdit::singleline(&mut self.distribute_dir)
                    .desired_width(420.0)
                    .hint_text("e.g. C:\\landesk-incoming"),
            );
        });
        ui.add_space(4.0);
        ui.label("Files to push (one local path per line):");
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
        // 先在本机校验路径：猜错了才在这里报，而不是等远端失败。
        let missing: Vec<&String> = files
            .iter()
            .filter(|path| !std::path::Path::new(path.as_str()).is_file())
            .collect();
        if !missing.is_empty() {
            ui.colored_label(
                egui::Color32::YELLOW,
                format!("Not found on this machine: {}", missing.iter().map(|p| p.as_str()).collect::<Vec<_>>().join(", ")),
            );
        }
        let can_start = !running && !self.targets.is_empty() && !files.is_empty() && missing.is_empty();
        if ui
            .add_enabled(can_start, egui::Button::new("Start"))
            .on_disabled_hover_text("Needs at least one target and one existing file")
            .clicked()
        {
            let peers: Vec<String> = self.targets.iter().cloned().collect();
            self.log.push(format!(
                "Pushing {} file(s) to {} machine(s)…",
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
            ui.label("running…");
        }

        ui.add_space(10.0);
        ui.separator();
        match &self.distribution {
            Some(distribution) => {
                for report in distribution.snapshot() {
                    let state = if report.failed > 0 {
                        format!(
                            "failed {} / {}: {}",
                            report.failed,
                            report.total,
                            report.last_error.unwrap_or_default()
                        )
                    } else {
                        format!("{}/{} done", report.done, report.total)
                    };
                    ui.horizontal(|ui| {
                        ui.monospace(&report.peer);
                        ui.label(state);
                    });
                }
            }
            None => {
                ui.label("Nothing pushed yet.");
            }
        }
    }

    fn draw_remote(&mut self, ui: &mut egui::Ui) {
        let Some(texture) = self.texture.clone() else {
            ui.label("Connected, waiting for the first frame…");
            return;
        };
        let (rw, rh) = (self.remote_size.0 as f32, self.remote_size.1 as f32);
        let available = ui.available_size();
        let scale = (available.x / rw).min(available.y / rh).min(1.0);
        let size = egui::vec2(rw * scale, rh * scale);
        let (rect, response) =
            ui.allocate_exact_size(size, egui::Sense::click_and_drag());
        // 点画面就把焦点抢回来：否则侧边栏里编辑过的文本框仍持有焦点，键盘输入会进那个框、不会转发到远端。
        if response.clicked() || response.drag_started() {
            response.request_focus();
        }
        // egui 0.31 里“窗口是否启用 IME”只看 `platform_output.ime` 是否为 Some（egui-winit 的
        // `allow_ime = ime.is_some()`），而只有 TextEdit 会设置它。远端画面不是 TextEdit，
        // 不自己置位的话输入法在本窗口里根本无法组合中文（点地址簿输入框能打、点画面就变英文）。
        // 只有“侧边栏文本框持有焦点”时才让位，否则我们总是把 IME 指向画面。
        let text_field_focused = ui.memory(|m| m.focused()).is_some_and(|id| !response.has_focus() && id != response.id);
        if !text_field_focused {
            ui.ctx().output_mut(|o| {
                o.ime = Some(egui::output::IMEOutput {
                    rect,
                    cursor_rect: egui::Rect::from_min_size(rect.min, egui::vec2(1.0, 1.0)),
                });
            });
        }
        ui.painter().image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );

        let Some(session) = &self.session else {
            return;
        };
        let (modifiers, scroll, events, left_pressed, left_released, right_pressed, right_released, pointer) =
            ui.input(|i| {
                (
                    i.modifiers,
                    i.raw_scroll_delta.y,
                    i.events.clone(),
                    i.pointer.button_pressed(egui::PointerButton::Primary),
                    i.pointer.button_released(egui::PointerButton::Primary),
                    i.pointer.button_pressed(egui::PointerButton::Secondary),
                    i.pointer.button_released(egui::PointerButton::Secondary),
                    i.pointer.latest_pos(),
                )
            });
        let (alt, ctrl, shift, command) = (
            modifiers.alt,
            modifiers.ctrl,
            modifiers.shift,
            modifiers.command,
        );
        // 修饰键：Ctrl/Shift/Alt 的状态变化要真的发过去，否则 Ctrl+C 这类组合在远端无效。
        // 注意 `command` 在 Windows 上就是 Ctrl（egui 的约定），把它当 Meta 发出去会让远端
        // 收到 Windows 键——Ctrl+V 就变成了弹开始菜单。
        let mut transitions = vec![
            (modifiers.ctrl, self.last_modifiers.ctrl, ControlKey::Control),
            (modifiers.shift, self.last_modifiers.shift, ControlKey::Shift),
            (modifiers.alt, self.last_modifiers.alt, ControlKey::Alt),
        ];
        #[cfg(target_os = "macos")]
        transitions.push((
            modifiers.command,
            self.last_modifiers.command,
            ControlKey::Meta,
        ));
        for (now, before, key) in transitions {
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
        // 按键位在上游是「按键 << 3 | 类型」，低三位留给类型。
        let click = |mask: i32, pos: egui::Pos2| {
            let (x, y) = to_remote(pos);
            session.send_mouse(mask, x, y, alt, ctrl, shift, command);
        };
        if pointer.map(|p| rect.contains(p)).unwrap_or(false) {
            if let Some(pos) = pointer {
                if left_pressed {
                    click((MOUSE_BUTTON_LEFT << 3) | MOUSE_TYPE_DOWN, pos);
                }
                if left_released {
                    click((MOUSE_BUTTON_LEFT << 3) | MOUSE_TYPE_UP, pos);
                }
                if right_pressed {
                    click((MOUSE_BUTTON_RIGHT << 3) | MOUSE_TYPE_DOWN, pos);
                }
                if right_released {
                    click((MOUSE_BUTTON_RIGHT << 3) | MOUSE_TYPE_UP, pos);
                }
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
        // 按住 Ctrl/Alt 时 egui 不再产生文本事件（它把这些当快捷键），所以这时要靠
        // `Event::Key` 把字母/数字本身发过去，否则远端只收到一个孤零零的 Ctrl。
        let shortcut_mode = ctrl || alt;
        // 一个字符变成一个 `chr` 事件对（down + up，与上游同形）：目标机按自己的布局/Unicode 注入。
        let send_text = |text: &str| {
            for ch in text.chars() {
                for down in [true, false] {
                    let mut evt = KeyEvent::new();
                    evt.set_chr(ch as u32);
                    evt.down = down;
                    evt.press = down;
                    evt.mode = KeyboardMode::Legacy.into();
                    // 远端靠这个字段判断“这是不是组合键”（has_hotkey_modifiers）。
                    evt.modifiers = held_modifiers(alt, ctrl, shift);
                    session.send_key(&evt);
                }
            }
        };
        for event in events {
            match event {
                // 可打印字符走 legacy 模式的 unicode 通道：目标机自己按自己的键盘布局查键码。
                egui::Event::Text(text) => {
                    if shortcut_mode {
                        continue;
                    }
                    send_text(&text);
                }
                // 输入法组合完成的字走的是**这里**而不是 `Event::Text`（egui-winit 的
                // `Ime::Commit` → `Event::Ime(ImeEvent::Commit)`），不处理就等于中文全被丢掉。
                egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                    if shortcut_mode {
                        continue;
                    }
                    send_text(&text);
                }
                // 正在组合中的预编辑文本不能发给远端，否则会把拼音也打出去。
                egui::Event::Ime(_) => {}
                egui::Event::Key {
                    key,
                    pressed,
                    repeat: _,
                    modifiers: _,
                    physical_key: _,
                } => {
                    if let Some(control_key) = control_key_of(key) {
                        let mut evt = KeyEvent::new();
                        evt.set_control_key(control_key);
                        evt.down = pressed;
                        evt.press = pressed;
                        session.send_key(&evt);
                    } else if shortcut_mode && pressed {
                        // Ctrl+V / Ctrl+C 这类组合：把字母当按键发，远端的注入才带得上修饰键。
                        if let Some(ch) = key_char(key) {
                            let mut evt = KeyEvent::new();
                            evt.set_chr(ch as u32);
                            evt.down = true;
                            evt.press = true;
                            evt.mode = KeyboardMode::Legacy.into();
                            evt.modifiers = held_modifiers(alt, ctrl, shift);
                            session.send_key(&evt);
                        }
                    }
                }
                // egui-winit 把 Ctrl+C / Ctrl+X / Ctrl+V 吞成 Copy / Cut / Paste，不再发 `Event::Key`。
                // 这三个要自己组回组合键发给远端，否则 Ctrl+A 能用、复制粘贴没反应。
                egui::Event::Copy => send_shortcut(&session, 'c', alt, shift),
                egui::Event::Cut => send_shortcut(&session, 'x', alt, shift),
                egui::Event::Paste(_) => send_shortcut(&session, 'v', alt, shift),
                _ => {}
            }
        }
        let _ = key_event::Union::Chr(0); // 保持对生成类型路径的引用，避免 cfg 差异导致未用告警
        let _ = MOUSE_BUTTON_RIGHT;
    }
}

/// 发一个 Ctrl+<键> 组合（egui 不给我们按键事件时用）。
fn send_shortcut(session: &SessionHandle, ch: char, alt: bool, shift: bool) {
    // 上游客户端对每个按键发一对事件（down 然后 up）；只发 down 会让远端的这个键永远“按着”，
    // 资源管理器这类应用要求完整的一次按键循环才会真正执行复制/剪切/粘贴。
    let modifiers = held_modifiers(alt, true, shift);
    for down in [true, false] {
        let mut evt = KeyEvent::new();
        evt.set_chr(ch as u32);
        evt.down = down;
        evt.press = down;
        evt.mode = KeyboardMode::Legacy.into();
        evt.modifiers = modifiers.clone();
        session.send_key(&evt);
    }
    hbb_common::log::debug!("landesk: sent Ctrl+{ch} as a key pair (modifiers: {})", modifiers.len());
}

/// 当前按着哪些修饰键（远端据此判定组合键）。
fn held_modifiers(
    alt: bool,
    ctrl: bool,
    shift: bool,
) -> Vec<hbb_common::protobuf::EnumOrUnknown<ControlKey>> {
    let mut held = Vec::new();
    if ctrl {
        held.push(ControlKey::Control.into());
    }
    if shift {
        held.push(ControlKey::Shift.into());
    }
    if alt {
        held.push(ControlKey::Alt.into());
    }
    held
}

/// 字母/数字/常见标点的字符形式，给“带修饰键的组合”用。
fn key_char(key: egui::Key) -> Option<char> {
    use egui::Key;
    Some(match key {
        Key::A => 'a',
        Key::B => 'b',
        Key::C => 'c',
        Key::D => 'd',
        Key::E => 'e',
        Key::F => 'f',
        Key::G => 'g',
        Key::H => 'h',
        Key::I => 'i',
        Key::J => 'j',
        Key::K => 'k',
        Key::L => 'l',
        Key::M => 'm',
        Key::N => 'n',
        Key::O => 'o',
        Key::P => 'p',
        Key::Q => 'q',
        Key::R => 'r',
        Key::S => 's',
        Key::T => 't',
        Key::U => 'u',
        Key::V => 'v',
        Key::W => 'w',
        Key::X => 'x',
        Key::Y => 'y',
        Key::Z => 'z',
        Key::Num0 => '0',
        Key::Num1 => '1',
        Key::Num2 => '2',
        Key::Num3 => '3',
        Key::Num4 => '4',
        Key::Num5 => '5',
        Key::Num6 => '6',
        Key::Num7 => '7',
        Key::Num8 => '8',
        Key::Num9 => '9',
        Key::Comma => ',',
        Key::Period => '.',
        Key::Semicolon => ';',
        Key::Slash => '/',
        Key::Minus => '-',
        Key::Plus => '+',
        Key::Equals => '=',
        Key::Quote => '\'',
        Key::Backslash => '\\',
        Key::Backtick => '`',
        Key::OpenBracket => '[',
        Key::CloseBracket => ']',
        _ => return None,
    })
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
        // 远程窗口模式：只有会话界面，没有地址簿。
        let remote = self.remote_addr.is_some();
        if !remote {
            self.poll_book_reload();
        } else {
            // 十几个会话窗口同时开着时，后台窗口不该和前台争 CPU：
            // 焦点变化 → 进程优先级 + 请求帧率一起降级/恢复。
            let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
            if self.last_focused != Some(focused) {
                self.last_focused = Some(focused);
                if let Some(session) = &self.session {
                    session.set_background(!focused, self.fps);
                }
            }
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if remote {
                    ui.heading(
                        self.remote_addr
                            .clone()
                            .unwrap_or_else(|| "lanDesk".to_owned()),
                    );
                    ui.separator();
                    if let Some(peer) = &self.session_peer {
                        let (state, _) = self.pin_state(peer);
                        ui.label(format!("Host: {state}"));
                    }
                    if ui.button("Disconnect").clicked() {
                        self.disconnect();
                    }
                    if ui.button("Fullscreen").clicked() {
                        let now = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!now));
                    }
                } else {
                    ui.heading("lanDesk");
                    ui.separator();
                    if ui.button("Reload address book").clicked() {
                        self.reload();
                    }
                    ui.separator();
                    ui.selectable_value(&mut self.show_distribute, false, "Address book");
                    ui.selectable_value(&mut self.show_distribute, true, "File push");
                    ui.separator();
                    ui.label(format!("Book: {}", self.book_path.display()));
                }
            });
            if let Some(err) = &self.load_error {
                ui.colored_label(egui::Color32::RED, format!("Reading the address book failed: {err}"));
            }
            ui.add_space(4.0);
        });

        if !remote {
        egui::SidePanel::left("tree")
            .resizable(true)
            .default_width(280.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                if self.book.entries().is_empty() {
                    ui.label("The address book is empty.");
                    ui.label("Import a list from the command line:");
                    ui.monospace("rustdesk.exe --landesk-book-import hosts.csv");
                    return;
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let tree = self.tree();
                    for (group, subgroups) in tree {
                        let group_name = if group.is_empty() {
                            "(no group)".to_owned()
                        } else {
                            group.clone()
                        };
                        egui::CollapsingHeader::new(group_name)
                            .default_open(true)
                            .show(ui, |ui| {
                                for (subgroup, entries) in subgroups {
                                    let subgroup_name = if subgroup.is_empty() {
                                        "(no subgroup)".to_owned()
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
                                                // 没 presence（无 rendezvous）就不假装在线：
                                                // 只如实显示指纹是否已核对 + 最近一次成功连接。
                                                // 状态用**画出来的圆点**而不是字符：中文字体常常缺 `✓`/`·`，
                                                // 会显示成豆腐块，而图形与字体无关。
                                                let (color, tip) = match self.book.pins().get(&entry.addr) {
                                                    Some(host) if host.verified => (
                                                        egui::Color32::from_rgb(60, 180, 90),
                                                        format!("verified {}", host.fingerprint),
                                                    ),
                                                    Some(host) => (
                                                        egui::Color32::from_rgb(220, 170, 40),
                                                        format!("unverified {}", host.fingerprint),
                                                    ),
                                                    None => (
                                                        egui::Color32::from_gray(120),
                                                        "never connected".to_owned(),
                                                    ),
                                                };
                                                ui.horizontal(|ui| {
                                                    let mut is_target =
                                                        self.targets.contains(&entry.addr);
                                                    if ui
                                                        .checkbox(&mut is_target, "")
                                                        .on_hover_text("Target for file push")
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
                                                    let response = ui
                                                        .selectable_label(selected, label)
                                                        .on_hover_text(tip.clone());
                                                    if response.clicked() {
                                                        self.selected = Some(entry.addr.clone());
                                                    }
                                                    // QQ 习惯：双击直接开会话窗口。
                                                    if response.double_clicked() {
                                                        self.selected = Some(entry.addr.clone());
                                                        open_remote_window(&entry.addr);
                                                    }
                                                    let (rect, dot) = ui.allocate_exact_size(
                                                        egui::vec2(10.0, 10.0),
                                                        egui::Sense::hover(),
                                                    );
                                                    ui.painter().circle_filled(rect.center(), 4.0, color);
                                                    dot.on_hover_text(tip);
                                                });
                                            }
                                        });
                                }
                            });
                    }
                });
            });
        }

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
                    ui.label(format!("Remote: {peer}"));
                    let (state, _) = self.pin_state(&peer);
                    ui.label(format!("Host fingerprint: {state}"));
                    // 对端有不止一块屏时给个切换；切换后等新屏的第一帧。
                    let displays = self
                        .session
                        .as_ref()
                        .map(|session| session.displays())
                        .unwrap_or_default();
                    if displays.len() > 1 {
                        ui.separator();
                        ui.label("Display:");
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
                    // 画质与帧率：真实网络里这决定好不好用。
                    ui.separator();
                    ui.label("Quality:");
                    for (value, label) in [
                        ("low", "low"),
                        ("balanced", "balanced"),
                        ("best", "best"),
                        ("custom", "custom"),
                    ] {
                        if ui.selectable_label(self.quality == value, label).clicked() {
                            self.quality = value.to_owned();
                            if let Some(session) = &self.session {
                                session.set_image_quality(value);
                            }
                        }
                    }
                    if self.quality == "custom" {
                        // 上游的自定义画质是“百分比 + 帧率上限”两个参数，缺一不可。
                        let mut percentage = self
                            .session
                            .as_ref()
                            .map(|session| session.custom_image_quality())
                            .unwrap_or(50);
                        ui.horizontal(|ui| {
                            ui.label("Custom quality:");
                            if ui
                                .add(egui::Slider::new(&mut percentage, 10..=100).suffix("%"))
                                .changed()
                            {
                                if let Some(session) = &self.session {
                                    session.set_custom_image_quality(percentage);
                                }
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label("FPS cap:");
                            for fps in [15, 30, 60] {
                                if ui
                                    .selectable_label(self.fps == fps, fps.to_string())
                                    .clicked()
                                    && self.fps != fps
                                {
                                    self.fps = fps;
                                    if let Some(session) = &self.session {
                                        session.set_fps(fps);
                                    }
                                }
                            }
                        });
                    }
                    // 编码器：上游的 codec-preference（下一次连接生效）。
                    ui.separator();
                    ui.label("Codec:");
                    for value in ["auto", "vp8", "vp9", "av1"] {
                        let current = self
                            .session
                            .as_ref()
                            .map(|s| s.codec_preference())
                            .unwrap_or_default();
                        if ui.selectable_label(current == value, value).clicked() {
                            if let Some(session) = &self.session {
                                session.set_codec_preference(value);
                            }
                            self.log
                                .push(format!("Codec preference set to {value} (takes effect on the next connect)"));
                        }
                    }
                });
                // 会话开关：声音、剪贴板、剪贴板传文件。
                ui.horizontal(|ui| {
                    let Some(session) = self.session.clone() else {
                        return;
                    };
                    let mut audio = session.audio_enabled();
                    if ui.checkbox(&mut audio, "Sound").changed() {
                        session.set_audio_enabled(audio);
                    }
                    let mut clipboard = session.clipboard_enabled();
                    if ui.checkbox(&mut clipboard, "Text clipboard").changed() {
                        session.set_clipboard_enabled(clipboard);
                    }
                    let mut file_clipboard = session.file_clipboard_enabled();
                    if ui.checkbox(&mut file_clipboard, "File clipboard").changed() {
                        session.set_file_clipboard(file_clipboard);
                    }
                });
                ui.separator();
                self.draw_remote(ui);
                return;
            }

            let Some(addr) = self.selected.clone() else {
                ui.label("Pick a machine on the left.");
                ui.add_space(6.0);
                self.draw_first_run_hints(ui);
                return;
            };

            // 编辑中：就地显示表单。
            if let Some(mut draft) = self.editing.clone() {
                ui.add_space(6.0);
                ui.heading(if draft.addr.is_empty() {
                    "New machine".to_owned()
                } else {
                    format!("Edit {}", draft.addr)
                });
                egui::Grid::new("entry-form")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Group");
                        ui.text_edit_singleline(&mut draft.group);
                        ui.end_row();
                        ui.label("Subgroup");
                        ui.text_edit_singleline(&mut draft.subgroup);
                        ui.end_row();
                        ui.label("Display name");
                        ui.text_edit_singleline(&mut draft.name);
                        ui.end_row();
                        ui.label("addr:port");
                        ui.text_edit_singleline(&mut draft.addr);
                        ui.end_row();
                        ui.label("Identity");
                        let mut key = draft.key.clone().unwrap_or_default();
                        if ui
                            .text_edit_singleline(&mut key)
                            .on_hover_text("client 私钥（身份名或路径）；留空 = 默认身份")
                            .changed()
                        {
                            draft.key = if key.trim().is_empty() {
                                None
                            } else {
                                Some(key.trim().to_owned())
                            };
                        }
                        ui.end_row();
                    });
                self.editing = Some(draft.clone());
                ui.add_space(6.0);
                let valid = draft.addr.contains(':') && !draft.addr.trim().is_empty();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(valid, egui::Button::new("Save"))
                        .clicked()
                    {
                        self.book.upsert(draft.clone());
                        match self.book.save(&self.book_path) {
                            Ok(()) => {
                                self.log.push(format!("Saved entry {}", draft.addr));
                                self.selected = Some(draft.addr.clone());
                                self.editing = None;
                                self.reload();
                            }
                            Err(err) => self.log.push(format!("Saving the address book failed: {err:#}")),
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        self.editing = None;
                    }
                    if !valid {
                        ui.small("Address needs a port, e.g. 10.0.0.5:21118");
                    }
                });
                return;
            }
            let Some(entry) = self.book.get(&addr).cloned() else {
                return;
            };
            ui.add_space(6.0);
            ui.heading(if entry.name.is_empty() {
                addr.clone()
            } else {
                entry.name.clone()
            });
            ui.label(format!("Address: {}", entry.addr));
            let (state, can_verify) = self.pin_state(&addr);
            ui.label(format!("Host fingerprint: {state}"));
            let identity = match &entry.key {
                Some(name) => match librustdesk::landesk::resolve_identity(name) {
                    Some(path) => format!("{} ({})", name, path.display()),
                    None => format!("{} — NOT FOUND in identities", name),
                },
                None => match librustdesk::landesk::default_identity() {
                    Some(path) => format!("default ({})", path.display()),
                    None => "none — generate or import a key first".to_owned(),
                },
            };
            ui.label(format!("Identity: {identity}"));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .button("Open remote window")
                    .on_hover_text("Opens a separate window; closing it disconnects")
                    .clicked()
                {
                    open_remote_window(&addr);
                }
        
                if can_verify && ui.button("I checked it, mark verified").clicked() {
                    self.mark_verified(&addr);
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Edit").clicked() {
                    self.editing = Some(entry.clone());
                }
                if ui
                    .button("Delete")
                    .on_hover_text("Also drops its fingerprint; the next connect starts trust over")
                    .clicked()
                {
                    let removed = self.book.remove(&addr);
                    match self.book.save(&self.book_path) {
                        Ok(()) if removed => {
                            self.log.push(format!("Deleted entry {addr}"));
                            self.selected = None;
                            self.reload();
                        }
                        Ok(()) => self.log.push(format!("{addr} is not in the address book")),
                        Err(err) => self.log.push(format!("Saving the address book failed: {err:#}")),
                    }
                }
                if ui.button("Add entry").clicked() {
                    self.editing = Some(Entry::default());
                }
            });
            ui.add_space(12.0);
            ui.separator();
            ui.label(format!("Private key: {}", self.key_path.display()));
            ui.small("Key and address book live beside this exe; use --landesk-key / --landesk-book to move them.");
        });    }
}

/// 文件剪贴板的 Ctrl+V 兜底。
///
/// egui-winit 处理 Ctrl+V 时只读取**文本**剪贴板：剪贴板里是文件时它取不到文本，
/// 于是既不产生 `Event::Paste`，也把 V 键事件吞掉（`egui-winit/src/lib.rs:772`），
/// 上层完全看不到这次按键。这里用 Win32 的键状态轮询补上这一种情况：
/// 只在"剪贴板含文件且不含文本"时触发，因此不会和 egui 的文本粘贴重复发送。
mod paste_watcher {
    use super::send_shortcut;
    use librustdesk::landesk::session::SessionHandle;
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    fn active() -> &'static Mutex<Option<SessionHandle>> {
        static ACTIVE: OnceLock<Mutex<Option<SessionHandle>>> = OnceLock::new();
        ACTIVE.get_or_init(|| Mutex::new(None))
    }

    pub fn set(session: Option<SessionHandle>) {
        if let Ok(mut slot) = active().lock() {
            *slot = session;
        }
    }

    pub fn spawn() {
        std::thread::spawn(|| {
            let mut was_down = false;
            loop {
                std::thread::sleep(Duration::from_millis(30));
                let down = key_down(VK_CONTROL) && key_down(VK_V);
                if down && !was_down && files_without_text_on_clipboard() {
                    let session = active().lock().ok().and_then(|slot| slot.clone());
                    if let Some(session) = session {
                        hbb_common::log::debug!("landesk: forwarding Ctrl+V for a file clipboard");
                        send_shortcut(&session, 'v', false, false);
                    }
                }
                was_down = down;
            }
        });
    }

    const VK_CONTROL: i32 = 0x11;
    const VK_V: i32 = 0x56;
    const CF_UNICODETEXT: u32 = 13;
    const CF_HDROP: u32 = 15;

    extern "system" {
        fn GetAsyncKeyState(vkey: i32) -> i16;
        fn IsClipboardFormatAvailable(format: u32) -> i32;
    }

    fn key_down(vkey: i32) -> bool {
        // 最高位为 1 表示当前按着
        (unsafe { GetAsyncKeyState(vkey) } as u16) & 0x8000 != 0
    }

    fn files_without_text_on_clipboard() -> bool {
        unsafe {
            IsClipboardFormatAvailable(CF_HDROP) != 0 && IsClipboardFormatAvailable(CF_UNICODETEXT) == 0
        }
    }
}

/// 打开一个独立的远程会话窗口：另起一个本程序的进程（`--landesk-remote <addr>`）。
///
/// 与上游 RustDesk 的“每个会话一个进程”同形：窗口关掉就等于断开会话，
/// 一个窗口崩溃不影响其它窗口，剪贴板/输入法也天然按窗口隔离。
fn open_remote_window(addr: &str) {
    let exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(err) => {
            hbb_common::log::error!("landesk: cannot find our own exe: {err}");
            return;
        }
    };
    match std::process::Command::new(exe)
        .arg("--landesk-remote")
        .arg(addr)
        .spawn()
    {
        Ok(_) => hbb_common::log::info!("landesk: opened a remote window for {addr}"),
        Err(err) => hbb_common::log::error!("landesk: cannot open a remote window for {addr}: {err}"),
    }
}

