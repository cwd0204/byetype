//! 单独修饰键（右 ⌥ / 右 ⌘ 等）作为全局快捷键的监听路径。
//!
//! `tauri-plugin-global-shortcut` 底层的 global-hotkey 只认「修饰键 + 主键」，纯修饰键
//! 注册不了（Carbon `RegisterEventHotKey` 同样不支持）。这里改用轮询读键状态，每 15 ms
//! 采样一次做边沿检测。不需要额外权限，不受输入法影响。
//!
//! 怎么读「某个修饰键是否按下」两个平台不一样：
//! - macOS 用 `CGEventSourceFlagsState` 的 NX_DEVICE*KEYMASK 设备位。**不能用
//!   `CGEventSourceKeyState`**：实测按右 Option 时它只让左 Option 的键码 58 变真，
//!   键码 61 永远不亮（右 Command 同样报成 55），也就是它分不清左右修饰键
//! - Windows 用 `GetAsyncKeyState`，VK_LMENU / VK_RMENU 这组虚拟键码本身区分左右
//!
//! 按住修饰键期间若有别的键被按下，说明用户在打组合键（比如按住 ⌥ 输入 ∂），松开时
//! 发 `Cancelled` 而不是 `Released`，由调用方决定丢弃。
//!
//! 存储值直接用浏览器 `KeyboardEvent.code` 的 8 个字面值（`AltRight` 等），前端
//! `GeneralTab.tsx` 用同一组字符串，两边不需要再做映射。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

/// 有绑定时的采样间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(15);
/// 没有任何绑定时的空转间隔。
const IDLE_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModifierKey {
    AltLeft,
    AltRight,
    MetaLeft,
    MetaRight,
    ControlLeft,
    ControlRight,
    ShiftLeft,
    ShiftRight,
}

impl ModifierKey {
    pub const ALL: [ModifierKey; 8] = [
        ModifierKey::AltLeft,
        ModifierKey::AltRight,
        ModifierKey::MetaLeft,
        ModifierKey::MetaRight,
        ModifierKey::ControlLeft,
        ModifierKey::ControlRight,
        ModifierKey::ShiftLeft,
        ModifierKey::ShiftRight,
    ];

    /// 识别配置里的快捷键字符串是否为单独修饰键；不是则交给 global-shortcut 解析。
    pub fn parse(value: &str) -> Option<Self> {
        ModifierKey::ALL
            .iter()
            .copied()
            .find(|key| key.name() == value.trim())
    }

    /// 与前端 `KeyboardEvent.code` 一致的名字，也是 config.json 里的存储值。
    pub fn name(self) -> &'static str {
        match self {
            ModifierKey::AltLeft => "AltLeft",
            ModifierKey::AltRight => "AltRight",
            ModifierKey::MetaLeft => "MetaLeft",
            ModifierKey::MetaRight => "MetaRight",
            ModifierKey::ControlLeft => "ControlLeft",
            ModifierKey::ControlRight => "ControlRight",
            ModifierKey::ShiftLeft => "ShiftLeft",
            ModifierKey::ShiftRight => "ShiftRight",
        }
    }
}

/// 交给调用方的事件。`Cancelled` 表示按住期间夹带了别的键，这次按放不算快捷键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Pressed,
    Released,
    Cancelled,
}

pub type Handler = Arc<dyn Fn(HotkeyEvent) + Send + Sync>;

/// 边沿检测：把「当前是否按下 / 是否有其他键按下」的采样序列翻译成事件。
#[derive(Debug, Default)]
struct KeyTracker {
    held: bool,
    contaminated: bool,
}

impl KeyTracker {
    fn step(&mut self, down: bool, other_down: bool) -> Option<HotkeyEvent> {
        match (self.held, down) {
            (false, true) => {
                self.held = true;
                self.contaminated = other_down;
                Some(HotkeyEvent::Pressed)
            }
            (true, true) => {
                if other_down {
                    self.contaminated = true;
                }
                None
            }
            (true, false) => {
                self.held = false;
                let contaminated = std::mem::take(&mut self.contaminated);
                Some(if contaminated {
                    HotkeyEvent::Cancelled
                } else {
                    HotkeyEvent::Released
                })
            }
            (false, false) => None,
        }
    }
}

struct Binding {
    key: ModifierKey,
    handler: Handler,
    tracker: KeyTracker,
}

fn bindings() -> &'static Mutex<Vec<Binding>> {
    static BINDINGS: OnceLock<Mutex<Vec<Binding>>> = OnceLock::new();
    BINDINGS.get_or_init(|| Mutex::new(Vec::new()))
}

static POLLER_STARTED: AtomicBool = AtomicBool::new(false);

/// 整体替换当前绑定。传空列表即清空；首次拿到非空列表时启动唯一的轮询线程。
pub fn set_bindings(list: Vec<(ModifierKey, Handler)>) {
    let list: Vec<Binding> = list
        .into_iter()
        .map(|(key, handler)| Binding {
            key,
            handler,
            tracker: KeyTracker::default(),
        })
        .collect();
    let need_poller = !list.is_empty();
    *bindings().lock().unwrap_or_else(|e| e.into_inner()) = list;

    if need_poller && !POLLER_STARTED.swap(true, Ordering::SeqCst) {
        if let Err(e) = thread::Builder::new()
            .name("modifier-hotkey".into())
            .spawn(poll_loop)
        {
            POLLER_STARTED.store(false, Ordering::SeqCst);
            eprintln!("Failed to start modifier hotkey poller: {}", e);
        }
    }
}

fn poll_loop() {
    loop {
        // 采样与回调分开：handler 里可能会重新 register（进而调 set_bindings 拿锁）。
        let fired: Vec<(Handler, HotkeyEvent)> = {
            let mut guard = bindings().lock().unwrap_or_else(|e| e.into_inner());
            if guard.is_empty() {
                drop(guard);
                thread::sleep(IDLE_INTERVAL);
                continue;
            }
            let states: Vec<bool> = guard
                .iter()
                .map(|b| platform::is_modifier_down(b.key))
                .collect();
            // 只有在有修饰键按着时才去扫全键盘，空闲时每轮只有几次系统调用。
            let any_held = guard
                .iter()
                .zip(&states)
                .any(|(b, down)| b.tracker.held || *down);
            let other_down = any_held && platform::any_other_key_down();
            guard
                .iter_mut()
                .zip(states)
                .filter_map(|(b, down)| {
                    b.tracker
                        .step(down, other_down)
                        .map(|event| (b.handler.clone(), event))
                })
                .collect()
        };
        for (handler, event) in fired {
            handler(event);
        }
        thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::ModifierKey;

    // CoreGraphics 已由 core-graphics crate 链接，这里只声明用到的两个符号。
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceKeyState(state_id: i32, key: u16) -> bool;
        fn CGEventSourceFlagsState(state_id: i32) -> u64;
    }

    /// kCGEventSourceStateCombinedSessionState：当前登录会话里所有事件源的合并状态。
    const COMBINED_SESSION_STATE: i32 = 0;

    /// 不算作「打字」的键：8 个修饰键的键码、Fn(63)、CapsLock(57)。
    const IGNORED: &[u16] = &[54, 55, 56, 57, 58, 59, 60, 61, 62, 63];

    /// NX_DEVICE*KEYMASK：flags 里区分左右修饰键的设备位（IOKit `IOLLEvent.h`）。
    /// Option / Command 两组已用真键盘验证过。
    fn device_bit(key: ModifierKey) -> u64 {
        match key {
            ModifierKey::ControlLeft => 0x0000_0001,
            ModifierKey::ShiftLeft => 0x0000_0002,
            ModifierKey::ShiftRight => 0x0000_0004,
            ModifierKey::MetaLeft => 0x0000_0008,
            ModifierKey::MetaRight => 0x0000_0010,
            ModifierKey::AltLeft => 0x0000_0020,
            ModifierKey::AltRight => 0x0000_0040,
            ModifierKey::ControlRight => 0x0000_2000,
        }
    }

    pub fn is_modifier_down(key: ModifierKey) -> bool {
        let flags = unsafe { CGEventSourceFlagsState(COMBINED_SESSION_STATE) };
        flags & device_bit(key) != 0
    }

    pub fn any_other_key_down() -> bool {
        // 普通键用键码读没问题（只有修饰键分不清左右）
        (0u16..=0x7F)
            .filter(|code| !IGNORED.contains(code))
            .any(|code| unsafe { CGEventSourceKeyState(COMBINED_SESSION_STATE, code) })
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::ModifierKey;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

    /// 不算作「打字」的键：左右修饰键、不分左右的 VK_SHIFT/CONTROL/MENU、Win 键、
    /// CapsLock / NumLock / ScrollLock。鼠标键（0x01-0x06）在扫描范围之外。
    const IGNORED: &[u16] = &[
        0x10, 0x11, 0x12, 0x14, 0x5B, 0x5C, 0x90, 0x91, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5,
    ];

    /// Windows 虚拟键码本身区分左右。
    fn vk(key: ModifierKey) -> u16 {
        match key {
            ModifierKey::AltLeft => 0xA4,      // VK_LMENU
            ModifierKey::AltRight => 0xA5,     // VK_RMENU
            ModifierKey::MetaLeft => 0x5B,     // VK_LWIN
            ModifierKey::MetaRight => 0x5C,    // VK_RWIN
            ModifierKey::ControlLeft => 0xA2,  // VK_LCONTROL
            ModifierKey::ControlRight => 0xA3, // VK_RCONTROL
            ModifierKey::ShiftLeft => 0xA0,    // VK_LSHIFT
            ModifierKey::ShiftRight => 0xA1,   // VK_RSHIFT
        }
    }

    fn is_key_down(vk: u16) -> bool {
        // 高位为 1 表示当前按下
        unsafe { (GetAsyncKeyState(vk as i32) as u16) & 0x8000 != 0 }
    }

    pub fn is_modifier_down(key: ModifierKey) -> bool {
        is_key_down(vk(key))
    }

    pub fn any_other_key_down() -> bool {
        (0x08u16..=0xFE)
            .filter(|vk| !IGNORED.contains(vk))
            .any(is_key_down)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    use super::ModifierKey;

    pub fn is_modifier_down(_key: ModifierKey) -> bool {
        false
    }

    pub fn any_other_key_down() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_trips_all_names() {
        for key in ModifierKey::ALL {
            assert_eq!(ModifierKey::parse(key.name()), Some(key));
        }
        assert_eq!(
            ModifierKey::parse(" AltRight "),
            Some(ModifierKey::AltRight)
        );
    }

    #[test]
    fn parse_rejects_ordinary_shortcuts() {
        for value in [
            "F4",
            "Ctrl+Shift+D",
            "Alt",
            "Command",
            "Option",
            "",
            "altright",
        ] {
            assert_eq!(ModifierKey::parse(value), None, "{value:?}");
        }
    }

    #[test]
    fn clean_press_and_release() {
        let mut t = KeyTracker::default();
        assert_eq!(t.step(false, false), None);
        assert_eq!(t.step(true, false), Some(HotkeyEvent::Pressed));
        assert_eq!(t.step(true, false), None);
        assert_eq!(t.step(false, false), Some(HotkeyEvent::Released));
        assert_eq!(t.step(false, false), None);
    }

    #[test]
    fn other_key_while_held_cancels() {
        let mut t = KeyTracker::default();
        assert_eq!(t.step(true, false), Some(HotkeyEvent::Pressed));
        assert_eq!(t.step(true, true), None);
        assert_eq!(t.step(true, false), None);
        assert_eq!(t.step(false, false), Some(HotkeyEvent::Cancelled));
        // 污染标记只影响这一次按放
        assert_eq!(t.step(true, false), Some(HotkeyEvent::Pressed));
        assert_eq!(t.step(false, false), Some(HotkeyEvent::Released));
    }

    #[test]
    fn other_key_already_down_at_press_cancels() {
        let mut t = KeyTracker::default();
        assert_eq!(t.step(true, true), Some(HotkeyEvent::Pressed));
        assert_eq!(t.step(false, false), Some(HotkeyEvent::Cancelled));
    }

    /// 真机联调：3 秒内按一下右 Option，确认进程内能读到键状态。
    /// `cargo test --lib live_modifier_key_state -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_modifier_key_state() {
        let mut last = false;
        let mut transitions = 0;
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            let down = platform::is_modifier_down(ModifierKey::AltRight);
            if down != last {
                eprintln!("[live] AltRight {}", if down { "DOWN" } else { "up" });
                transitions += 1;
                last = down;
            }
            thread::sleep(POLL_INTERVAL);
        }
        assert!(
            transitions >= 2,
            "no right Option press observed within 3 s"
        );
    }

    #[test]
    fn set_bindings_replaces_previous_list() {
        let noop: Handler = Arc::new(|_| {});
        set_bindings(vec![(ModifierKey::AltRight, noop.clone())]);
        assert_eq!(bindings().lock().unwrap().len(), 1);
        set_bindings(Vec::new());
        assert!(bindings().lock().unwrap().is_empty());
    }
}
