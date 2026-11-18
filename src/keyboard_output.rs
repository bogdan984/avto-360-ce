//! Эмуляция клавиатуры через Win32 SendInput (скан-коды — для игр на
//! DirectInput/RawInput). Хранит список программно зажатых клавиш и
//! гарантированно отпускает их при сбросе и при уничтожении.

use crate::model::KeyOutput;
use std::mem::size_of;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MAPVK_VK_TO_VSC_EX,
};

const VK_LSHIFT: u16 = 0xA0;
const VK_LCONTROL: u16 = 0xA2;
const VK_LMENU: u16 = 0xA4;
const VK_LWIN: u16 = 0x5B;

#[derive(Default)]
pub struct KeyboardOutput {
    /// Зажатые нами клавиши в порядке нажатия.
    held: Vec<u16>,
}

impl KeyboardOutput {
    /// Привести зажатые клавиши к желаемому набору.
    pub fn sync(&mut self, desired: &[KeyOutput]) {
        let wanted = expand(desired);
        let (ups, downs) = plan(&self.held, &wanted);
        if ups.is_empty() && downs.is_empty() {
            return;
        }
        let mut inputs: Vec<INPUT> = ups.iter().map(|vk| key_input(*vk, true)).collect();
        inputs.extend(downs.iter().map(|vk| key_input(*vk, false)));
        unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                size_of::<INPUT>() as i32,
            );
        }
        self.held.retain(|vk| !ups.contains(vk));
        self.held.extend(downs);
    }

    /// Отпустить всё, что было зажато программно.
    pub fn release_all(&mut self) {
        self.sync(&[]);
    }
}

impl Drop for KeyboardOutput {
    fn drop(&mut self) {
        self.release_all();
    }
}

/// Список VK: сначала модификаторы (Ctrl/Shift/Alt/Win), затем клавиши.
fn expand(desired: &[KeyOutput]) -> Vec<u16> {
    let mut modifiers = Vec::new();
    let mut keys = Vec::new();
    for key in desired {
        for (on, vk) in [
            (key.ctrl, VK_LCONTROL),
            (key.shift, VK_LSHIFT),
            (key.alt, VK_LMENU),
            (key.win, VK_LWIN),
        ] {
            if on && !modifiers.contains(&vk) {
                modifiers.push(vk);
            }
        }
        if key.vk != 0 && !keys.contains(&key.vk) {
            keys.push(key.vk);
        }
    }
    keys.retain(|vk| !modifiers.contains(vk));
    modifiers.extend(keys);
    modifiers
}

/// Что отпустить (в обратном порядке нажатия) и что нажать.
fn plan(held: &[u16], wanted: &[u16]) -> (Vec<u16>, Vec<u16>) {
    let ups = held
        .iter()
        .rev()
        .filter(|vk| !wanted.contains(vk))
        .copied()
        .collect();
    let downs = wanted
        .iter()
        .filter(|vk| !held.contains(vk))
        .copied()
        .collect();
    (ups, downs)
}

fn key_input(vk: u16, up: bool) -> INPUT {
    let scan = unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC_EX) };
    let mut flags = if up { KEYEVENTF_KEYUP } else { 0 };
    let (w_vk, w_scan) = if scan == 0 {
        // Нет скан-кода — отправляем виртуальный код.
        (vk, 0)
    } else {
        flags |= KEYEVENTF_SCANCODE;
        if scan & 0xFF00 == 0xE000 || scan & 0xFF00 == 0xE100 {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        (0, (scan & 0xFF) as u16)
    };
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: w_vk,
                wScan: w_scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers_go_down_first_and_release_in_reverse() {
        let key = KeyOutput {
            vk: 0x52,
            ctrl: true,
            ..Default::default()
        };
        let wanted = expand(&[key]);
        assert_eq!(wanted, vec![VK_LCONTROL, 0x52]);
        let (ups, downs) = plan(&[], &wanted);
        assert!(ups.is_empty());
        assert_eq!(downs, vec![VK_LCONTROL, 0x52]);
        let (ups, downs) = plan(&wanted, &[]);
        assert_eq!(ups, vec![0x52, VK_LCONTROL]);
        assert!(downs.is_empty());
    }
}
