//! «Игровые устройства» (joy.cpl): открыть «Свойства» нужного контроллера и
//! увести окна за край экрана — как в X52 Launcher. Некоторые драйверы
//! (рули, Saitek/Logitech) отдают часть кнопок только пока открыты свойства.

use std::{
    ffi::c_void, os::windows::process::CommandExt, process::Command, ptr, thread, time::Duration,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HWND, LPARAM, RECT},
    System::{
        Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory},
        Memory::{
            VirtualAllocEx, VirtualFreeEx, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE,
        },
        Threading::{OpenProcess, PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE},
    },
    UI::Input::KeyboardAndMouse::IsWindowEnabled,
    UI::WindowsAndMessaging::{
        EnumChildWindows, EnumWindows, GetClassNameW, GetWindowRect, GetWindowTextW,
        GetWindowThreadProcessId, IsWindowVisible, PostMessageW, SendMessageW, SetWindowPos,
        SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    },
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const LVM_GETITEMCOUNT: u32 = 0x1004;
const LVM_SETITEMSTATE: u32 = 0x102B;
const LVM_GETITEMTEXTW: u32 = 0x1073;
const BM_CLICK: u32 = 0x00F5;
/// Смещение буфера текста внутри выделенной в чужом процессе памяти.
const TEXT_OFFSET: usize = 0x100;
const TEXT_CHARS: usize = 512;

/// Открыть свойства устройства `device_name` за краем экрана.
/// Возвращает PID процесса joy.cpl (для закрытия при выходе).
pub fn open(device_name: &str) -> Result<u32, String> {
    let needle = device_name.trim().to_lowercase();
    if needle.is_empty() {
        return Err("не задано имя устройства".to_owned());
    }
    // Окна joy.cpl, открытые до нас вручную, не трогаем. Окно, уже стоящее
    // за краем экрана, — наше (осталось от прошлого запуска): берём его.
    let before: Vec<HWND> = windows(None)
        .into_iter()
        .filter(|hwnd| class_name(*hwnd) == "#32770" && !is_away(*hwnd))
        .collect();
    let mut child = Command::new("rundll32.exe")
        .args(["shell32.dll,Control_RunDLL", "joy.cpl"])
        .spawn()
        .map_err(|error| format!("не удалось запустить joy.cpl: {error}"))?;

    // Окно ищем по содержимому (строка с устройством в списке), а не по PID:
    // диалог может оказаться в другом процессе, чем запущенный rundll32.
    // Первые 3 с — только точное совпадение имени, потом и нестрогое
    // (имя из SDL часто отличается от имени в списке joy.cpl).
    // Новые окна joy.cpl уводим за экран сразу, ещё до поиска устройства.
    let child_pid = child.id();
    let mut found = None;
    for attempt in 0..300 {
        hide_new_dialogs(&before, child_pid);
        found = find_device(&before, &needle, attempt >= 60);
        if found.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let Some((main, list, index)) = found else {
        // Закрываем то, что успело открыться, — иначе окно остаётся на экране.
        let mut pids: Vec<u32> = new_dialogs(&before, child_pid)
            .into_iter()
            .map(window_pid)
            .collect();
        pids.dedup();
        for pid in pids {
            close(pid);
        }
        let _ = child.kill();
        return Err(format!("«{device_name}» нет в списке игровых устройств"));
    };
    away(main);
    let dialog_pid = window_pid(main);
    let _ = list_view_remote(list, Some(index));
    thread::sleep(Duration::from_millis(300));

    let button = windows(Some(main)).into_iter().find(|hwnd| {
        class_name(*hwnd) == "Button" && {
            let text = window_text(*hwnd).replace('&', "");
            text.contains("Свойства") || text.contains("Properties")
        }
    });
    let Some(button) = button else {
        close(dialog_pid);
        let _ = child.kill();
        return Err("кнопка «Свойства» не найдена".to_owned());
    };

    let mut sheets = Vec::new();
    for attempt in 0..150 {
        // PostMessage: SendMessage ждал бы закрытия модального окна свойств.
        // Раз в секунду повторяем: кнопка включается не сразу после выбора строки.
        // После нажатия процесс не убиваем — это ломает драйверы до переподключения.
        if attempt % 10 == 0 && unsafe { IsWindowEnabled(button) } != 0 {
            let _ = list_view_remote(list, Some(index));
            unsafe { PostMessageW(button, BM_CLICK, 0, 0) };
        }
        sheets = windows(None)
            .into_iter()
            .filter(|hwnd| {
                *hwnd != main
                    && window_pid(*hwnd) == dialog_pid
                    && unsafe { IsWindowVisible(*hwnd) } != 0
            })
            .collect();
        if !sheets.is_empty() {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    for hwnd in &sheets {
        away(*hwnd);
    }
    // Ещё раз — окно свойств может само встать по центру уже после нас.
    thread::sleep(Duration::from_millis(500));
    away(main);
    for hwnd in &sheets {
        away(*hwnd);
    }
    if sheets.is_empty() {
        // Не оставляем joy.cpl висеть за экраном — он мешал следующим попыткам.
        close(dialog_pid);
        return Err("окно свойств не найдено".to_owned());
    }
    // Окна могут сами вернуться в центр (перерисовка, смена DPI, Alt+Tab):
    // пока процесс жив, держим все его окна за экраном.
    thread::spawn(move || keep_away(dialog_pid));
    Ok(dialog_pid)
}

/// Новые (открытые после `before`) диалоги joy.cpl.
fn new_dialogs(before: &[HWND], child_pid: u32) -> Vec<HWND> {
    windows(None)
        .into_iter()
        .filter(|hwnd| {
            !before.contains(hwnd)
                && class_name(*hwnd) == "#32770"
                && unsafe { IsWindowVisible(*hwnd) } != 0
                && (window_pid(*hwnd) == child_pid || {
                    let title = window_text(*hwnd).to_lowercase();
                    title.contains("игровые устройства") || title.contains("game controllers")
                })
        })
        .collect()
}

fn hide_new_dialogs(before: &[HWND], child_pid: u32) {
    for hwnd in new_dialogs(before, child_pid) {
        if !is_away(hwnd) {
            away(hwnd);
        }
    }
}

/// Пока у процесса есть окна — возвращает их за край экрана.
fn keep_away(pid: u32) {
    let mut missing = 0;
    while missing < 8 {
        let own: Vec<HWND> = windows(None)
            .into_iter()
            .filter(|hwnd| window_pid(*hwnd) == pid && unsafe { IsWindowVisible(*hwnd) } != 0)
            .collect();
        if own.is_empty() {
            missing += 1;
        } else {
            missing = 0;
        }
        for hwnd in own {
            if !is_away(hwnd) {
                away(hwnd);
            }
        }
        thread::sleep(Duration::from_millis(250));
    }
}

/// Закрыть joy.cpl: сначала мягко (WM_CLOSE через taskkill без /F —
/// первый закрывает свойства, второй сам joy.cpl), принудительно — в
/// последнюю очередь.
pub fn close(pid: u32) {
    for force in [false, false, false, true] {
        let pid = pid.to_string();
        let mut args = vec!["/PID", pid.as_str(), "/T"];
        if force {
            args.push("/F");
        }
        let status = Command::new("taskkill")
            .args(&args)
            .creation_flags(CREATE_NO_WINDOW)
            .output();
        match status {
            // 128 — процесса уже нет.
            Ok(output) if output.status.code() == Some(128) => break,
            Err(_) => break,
            _ => thread::sleep(Duration::from_millis(500)),
        }
    }
}

/// Закрыть все окна joy.cpl, стоящие за краем экрана (их открываем только мы).
pub fn close_away() {
    let mut pids: Vec<u32> = windows(None)
        .into_iter()
        .filter(|hwnd| class_name(*hwnd) == "#32770" && is_away(*hwnd))
        .map(window_pid)
        .collect();
    pids.sort_unstable();
    pids.dedup();
    for pid in pids {
        close(pid);
    }
}

fn find_device(before: &[HWND], needle: &str, loose: bool) -> Option<(HWND, HWND, usize)> {
    for window in windows(None) {
        if before.contains(&window)
            || class_name(window) != "#32770"
            || unsafe { IsWindowVisible(window) } == 0
        {
            continue;
        }
        for list in windows(Some(window)) {
            if class_name(list) != "SysListView32" {
                continue;
            }
            let Ok(names) = list_view_remote(list, None) else {
                continue;
            };
            let names: Vec<String> = names.iter().map(|name| name.to_lowercase()).collect();
            let exact = names.iter().position(|name| {
                !name.is_empty() && (name.contains(needle) || needle.contains(name.as_str()))
            });
            if let Some(index) =
                exact.or_else(|| loose.then(|| loose_match(&names, needle)).flatten())
            {
                return Some((window, list, index));
            }
        }
    }
    None
}

/// Нестрогий выбор: строка с наибольшим числом общих слов с `needle`,
/// а если общих слов нет — единственное реальное (не виртуальное) устройство.
fn loose_match(names: &[String], needle: &str) -> Option<usize> {
    let real: Vec<usize> = (0..names.len())
        .filter(|&index| {
            let name = &names[index];
            !name.is_empty()
                && !name.contains("xbox 360")
                && !name.contains("vigem")
                && !name.contains("virtual")
        })
        .collect();
    let words: Vec<&str> = needle
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.chars().count() >= 3)
        .collect();
    let common = |index: usize| {
        words
            .iter()
            .filter(|word| names[index].contains(*word))
            .count()
    };
    real.iter()
        .copied()
        .filter(|&index| common(index) > 0)
        .max_by_key(|&index| common(index))
        .or_else(|| (real.len() == 1).then(|| real[0]))
}

fn away(hwnd: HWND) {
    unsafe {
        SetWindowPos(
            hwnd,
            ptr::null_mut(),
            -32000,
            -32000,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

fn is_away(hwnd: HWND) -> bool {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let ok = unsafe { GetWindowRect(hwnd, &mut rect) } != 0;
    ok && rect.left <= -30000
}

unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> windows_sys::core::BOOL {
    let list = unsafe { &mut *(lparam as *mut Vec<HWND>) };
    list.push(hwnd);
    1
}

fn windows(parent: Option<HWND>) -> Vec<HWND> {
    let mut list: Vec<HWND> = Vec::new();
    let lparam = &mut list as *mut Vec<HWND> as LPARAM;
    unsafe {
        match parent {
            None => EnumWindows(Some(collect), lparam),
            Some(parent) => EnumChildWindows(parent, Some(collect), lparam),
        };
    }
    list
}

fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    let length = unsafe { GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

fn window_text(hwnd: HWND) -> String {
    let mut buffer = [0u16; 512];
    let length = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    pid
}

/// LVITEMW (x64): state @12, stateMask @16, pszText @24, cchTextMax @32.
fn lv_item(state: u32, text_address: usize) -> [u8; 88] {
    let mut item = [0u8; 88];
    item[12..16].copy_from_slice(&state.to_le_bytes());
    item[16..20].copy_from_slice(&state.to_le_bytes());
    item[24..32].copy_from_slice(&(text_address as u64).to_le_bytes());
    item[32..36].copy_from_slice(&(TEXT_CHARS as i32).to_le_bytes());
    item
}

/// Строки SysListView32 чужого процесса или выделение строки `select`.
/// LVITEMW кладём в память процесса списка (той же разрядности, что и мы).
fn list_view_remote(list: HWND, select: Option<usize>) -> Result<Vec<String>, ()> {
    let pid = window_pid(list);
    let process = unsafe {
        OpenProcess(
            PROCESS_VM_OPERATION | PROCESS_VM_READ | PROCESS_VM_WRITE,
            0,
            pid,
        )
    };
    if process.is_null() {
        return Err(());
    }
    let memory = unsafe {
        VirtualAllocEx(
            process,
            ptr::null(),
            0x1000,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        )
    };
    let result = if memory.is_null() {
        Err(())
    } else {
        let write = |bytes: &[u8]| unsafe {
            WriteProcessMemory(
                process,
                memory,
                bytes.as_ptr() as *const c_void,
                bytes.len(),
                ptr::null_mut(),
            ) != 0
        };
        if let Some(index) = select {
            // LVIS_FOCUSED | LVIS_SELECTED
            if write(&lv_item(3, 0)) {
                unsafe { SendMessageW(list, LVM_SETITEMSTATE, index, memory as LPARAM) };
            }
            Ok(Vec::new())
        } else {
            let count = unsafe { SendMessageW(list, LVM_GETITEMCOUNT, 0, 0) }.max(0) as usize;
            let mut names = Vec::with_capacity(count);
            for index in 0..count {
                if !write(&lv_item(0, memory as usize + TEXT_OFFSET)) {
                    break;
                }
                let length =
                    unsafe { SendMessageW(list, LVM_GETITEMTEXTW, index, memory as LPARAM) }
                        .clamp(0, TEXT_CHARS as isize) as usize;
                let mut buffer = vec![0u16; length];
                let read = unsafe {
                    ReadProcessMemory(
                        process,
                        (memory as usize + TEXT_OFFSET) as *const c_void,
                        buffer.as_mut_ptr() as *mut c_void,
                        length * 2,
                        ptr::null_mut(),
                    )
                };
                if read != 0 {
                    names.push(String::from_utf16_lossy(&buffer));
                }
            }
            Ok(names)
        }
    };
    unsafe {
        if !memory.is_null() {
            VirtualFreeEx(process, memory, 0, MEM_RELEASE);
        }
        CloseHandle(process);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::loose_match;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_lowercase()).collect()
    }

    #[test]
    fn loose_match_picks_common_words_or_single_real_device() {
        let list = names(&["Controller (XBOX 360 For Windows)", "ARTPLAYS V-1200 Wheel"]);
        assert_eq!(loose_match(&list, "v-1200 racing wheel"), Some(1));
        // Общих слов нет, но реальное устройство одно.
        assert_eq!(loose_match(&list, "usb gamepad"), Some(1));
        let two = names(&["Saitek X52 Flight Controller", "Logitech G29"]);
        assert_eq!(loose_match(&two, "usb gamepad"), None);
    }
}
