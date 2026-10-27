//! **快捷键壳**：把 GPUI 的按键翻译成 core 的 [`Chord`]。
//!
//! 键位本身一律不在这里定——那份表在 `jiandu_core::keys`（`Action::ALL`）。
//! 本模块只干一件容易出错的事：**框架键名 → 规范键名**的归一。
//!
//! 三个坑，改这里的人先看一眼：
//!   1. 按住 Shift 时 GPUI 给的 `key` 可能是大写（`"A"`），而 `Action` 的键位
//!      一律小写 → 这里统一 `.to_ascii_lowercase()`，否则 ⌘⇧A 永远匹配不上。
//!   2. macOS 的 ⌘ 在 GPUI 里叫 `modifiers.platform`（不是 `control`）——
//!      `control` 是真正的 ⌃ 键。两者都要认，但**不能合并**：⌘C 与 ⌃C 是两回事。
//!   3. 输入法组字期间 `keystroke.key` 会是 `"Process"` 之类，`Chord` 里落成
//!      无修饰的怪键，`KeyMap::resolve` 因为「裸键不认」自然会放过它。

use gpui::Keystroke;
use jiandu_core::keys::Chord;

/// GPUI 的一次按键 → core 的 [`Chord`]。
pub fn chord_of(ks: &Keystroke) -> Chord {
    Chord::new(
        ks.modifiers.platform,
        ks.modifiers.shift,
        ks.modifiers.alt,
        ks.modifiers.control,
        &ks.key,
    )
}

/// 这个键是不是「纯修饰键」（按下 ⌘ 本身、还没按字母）。
///
/// 改键时要把这种键放过——否则用户按下 ⌘⇧A 的过程中，先到的那个 ⌘ 就被
/// 当成新键位了。GPUI 把单独按下的修饰键报成 `"shift"` / `"cmd"` 等。
pub fn is_modifier_only(ks: &Keystroke) -> bool {
    matches!(
        ks.key.to_ascii_lowercase().as_str(),
        "shift" | "control" | "alt" | "platform" | "cmd" | "super" | "meta" | "function" | "fn"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Modifiers;

    fn ks(key: &str, m: Modifiers) -> Keystroke {
        Keystroke {
            modifiers: m,
            key: key.to_string(),
            key_char: None,
        }
    }

    #[test]
    fn shift_uppercase_key_still_matches_lowercase_binding() {
        let m = Modifiers {
            platform: true,
            shift: true,
            ..Default::default()
        };
        let c = chord_of(&ks("A", m));
        assert_eq!(c, Chord::parse("cmd+shift+a").unwrap());
        // 显示顺序照 macOS 惯例 ⌃⌥⇧⌘——修饰键在左、主键在右。
        assert_eq!(c.display(), "⇧⌘A");
    }

    #[test]
    fn control_is_not_command() {
        let m = Modifiers {
            control: true,
            ..Default::default()
        };
        let c = chord_of(&ks("c", m));
        assert!(c.ctrl && !c.cmd, "⌃C 不该被当成 ⌘C");
    }

    #[test]
    fn lone_modifier_is_ignored_by_capture() {
        let m = Modifiers {
            platform: true,
            ..Default::default()
        };
        assert!(is_modifier_only(&ks("cmd", m)));
        assert!(is_modifier_only(&ks("Shift", m)));
        assert!(!is_modifier_only(&ks("a", m)));
    }
}
