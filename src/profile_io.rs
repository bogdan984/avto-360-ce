use crate::model::ControllerProfile;
use crate::wide::str_wide_null;
use anyhow::{Context, Result};
use std::{ffi::OsString, fs, os::windows::ffi::OsStringExt, path::PathBuf};
use windows_sys::Win32::UI::Controls::Dialogs::{
    CommDlgExtendedError, GetOpenFileNameW, GetSaveFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST,
    OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

/// Фильтр для стандартного окна Windows: пары «название\0маска\0», в конце \0.
const FILTER: &str = "Профиль x360ce (*.json)\0*.json\0Все файлы (*.*)\0*.*\0";

pub fn export_profile(profile: &ControllerProfile) -> Result<Option<PathBuf>> {
    let default_name = format!("{}.x360ce.json", sanitize_filename(&profile.name));
    let Some(path) = choose_path(DialogKind::Save, &default_name)? else {
        return Ok(None);
    };

    let mut serialized =
        serde_json::to_vec_pretty(profile).context("не удалось сохранить профиль в JSON")?;
    serialized.push(b'\n');
    fs::write(&path, serialized)
        .with_context(|| format!("не удалось экспортировать профиль: {}", path.display()))?;
    Ok(Some(path))
}

pub fn import_profile() -> Result<Option<(PathBuf, ControllerProfile)>> {
    let Some(path) = choose_path(DialogKind::Open, "")? else {
        return Ok(None);
    };

    let bytes = fs::read(&path)
        .with_context(|| format!("не удалось прочитать файл профиля: {}", path.display()))?;
    let mut profile: ControllerProfile = serde_json::from_slice(&bytes)
        .with_context(|| format!("неверный файл профиля x360ce: {}", path.display()))?;
    if profile.name.trim().is_empty() {
        profile.name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(|stem| stem.trim_end_matches(".x360ce").to_owned())
            .filter(|stem| !stem.is_empty())
            .unwrap_or_else(|| "Импортированный профиль".to_owned());
    }
    profile.sanitize();
    Ok(Some((path, profile)))
}

#[derive(Clone, Copy)]
enum DialogKind {
    Open,
    Save,
}

/// Стандартное окно выбора файла Windows, модальное к окну программы.
fn choose_path(kind: DialogKind, default_name: &str) -> Result<Option<PathBuf>> {
    let mut file = vec![0u16; 4096];
    let name: Vec<u16> = default_name.encode_utf16().take(file.len() - 1).collect();
    file[..name.len()].copy_from_slice(&name);
    let filter: Vec<u16> = FILTER.encode_utf16().chain(std::iter::once(0)).collect();
    let extension = str_wide_null("json");
    let title = str_wide_null(match kind {
        DialogKind::Open => "Импорт профиля",
        DialogKind::Save => "Экспорт профиля",
    });

    let mut dialog = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: unsafe { GetForegroundWindow() },
        lpstrFilter: filter.as_ptr(),
        nFilterIndex: 1,
        lpstrFile: file.as_mut_ptr(),
        nMaxFile: file.len() as u32,
        lpstrTitle: title.as_ptr(),
        lpstrDefExt: extension.as_ptr(),
        Flags: OFN_EXPLORER
            | OFN_NOCHANGEDIR
            | OFN_PATHMUSTEXIST
            | match kind {
                DialogKind::Open => OFN_FILEMUSTEXIST,
                DialogKind::Save => OFN_OVERWRITEPROMPT,
            },
        ..Default::default()
    };

    let ok = unsafe {
        match kind {
            DialogKind::Open => GetOpenFileNameW(&mut dialog),
            DialogKind::Save => GetSaveFileNameW(&mut dialog),
        }
    };
    if ok == 0 {
        let error = unsafe { CommDlgExtendedError() };
        if error == 0 {
            return Ok(None); // пользователь нажал «Отмена»
        }
        anyhow::bail!("ошибка окна выбора файла (код {error:#x})");
    }

    let len = file.iter().position(|&c| c == 0).unwrap_or(file.len());
    Ok(Some(PathBuf::from(OsString::from_wide(&file[..len]))))
}

/// Имя файла из названия профиля: кириллица остаётся, запрещённые в
/// Windows символы заменяются на «-».
fn sanitize_filename(value: &str) -> String {
    let sanitized: String = value
        .trim()
        .chars()
        .map(|character| {
            if character.is_control() || r#"<>:"/\|?*"#.contains(character) {
                '-'
            } else {
                character
            }
        })
        .collect();
    let sanitized = sanitized.trim_matches(|c: char| c == '.' || c == ' ' || c == '-');
    if sanitized.is_empty() {
        "profile".to_owned()
    } else {
        sanitized.to_owned()
    }
}
