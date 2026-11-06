//! 会话进程的优先级：前台会话正常，后台会话降级。
//!
//! 一个 Master 可能同时开十几个远程窗口，每会话一个进程。后台窗口如果和前台争 CPU
//! （解码、像素转换、重绘），前台就会掉帧，所以：
//!
//! * **进程优先级**：前台 `NORMAL_PRIORITY_CLASS`，后台 `BELOW_NORMAL_PRIORITY_CLASS`。
//!   不用 `IDLE` —— 那会让重新激活的瞬间明显发木。
//! * **请求帧率**：后台把 `custom_fps` 压到 [`BACKGROUND_FPS`]，恢复时交还给界面当前的帧率上限
//!   （上限又被上游的自适应 `fps_control` 用作天花板，所以恢复后它会自己爬回去）。
//!
//! 界面只负责告诉我们焦点变了；这里不碰窗口本身。

/// 后台会话请求的帧率。上游 `fps_control` 把小于 5 的值当成非法并重置成 30，
/// 所以这里不能用 1–2 那种更省的值。
pub const BACKGROUND_FPS: i32 = 5;

#[cfg(windows)]
mod imp {
    use hbb_common::log;

    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn SetPriorityClass(handle: isize, class: u32) -> i32;
    }

    const NORMAL_PRIORITY_CLASS: u32 = 0x0000_0020;
    const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

    pub fn set_background(background: bool) {
        let class = if background {
            BELOW_NORMAL_PRIORITY_CLASS
        } else {
            NORMAL_PRIORITY_CLASS
        };
        let ok = unsafe { SetPriorityClass(GetCurrentProcess(), class) };
        if ok == 0 {
            log::debug!("landesk: SetPriorityClass failed (background: {background})");
        }
    }
}

#[cfg(not(windows))]
mod imp {
    /// 其它平台先不降级：这里没有和 Windows 对等的、按需可逆的优先级接口。
    pub fn set_background(_background: bool) {}
}

/// 把这个会话进程在“前台 / 后台”之间切换。
pub fn set_background(background: bool) {
    imp::set_background(background);
}
