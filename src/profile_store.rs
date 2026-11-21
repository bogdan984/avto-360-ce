//! Профили списка хранятся в папке `profiles`, по файлу на профиль:
//! `<имя профиля>.json`. Имя файла и есть имя профиля, поэтому профили можно
//! переименовывать и удалять прямо в папке — программа перечитывает её.
use crate::model::ControllerProfile;
use anyhow::{Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

const EXTENSION: &str = "json";

/// Имя, пригодное для файла Windows: кириллица остаётся, запрещённые
/// символы заменяются на «-».
pub fn clean_name(value: &str) -> String {
    let cleaned: String = value
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
    cleaned
        .trim_matches(|c: char| c == '.' || c == ' ' || c == '-')
        .to_owned()
}

/// В Windows имена файлов не различают регистр.
pub fn same_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

pub fn profile_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.{EXTENSION}"))
}

/// Все профили папки, по имени. Файл, который не читается (например, его
/// прямо сейчас пишут), заменяется прежней копией из `previous`.
pub fn load_all(dir: &Path, previous: &[ControllerProfile]) -> Vec<ControllerProfile> {
    let Ok(read_dir) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut profiles: Vec<ControllerProfile> = read_dir
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case(EXTENSION))
        })
        .filter_map(|path| {
            let name = path.file_stem()?.to_str()?.trim().to_owned();
            if name.is_empty() {
                return None;
            }
            let parsed = fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<ControllerProfile>(&bytes).ok());
            let mut profile = match parsed {
                Some(profile) => profile,
                None => previous.iter().find(|saved| saved.name == name)?.clone(),
            };
            profile.name = name;
            profile.device_guid.clear();
            profile.sanitize();
            Some(profile)
        })
        .collect();
    profiles.sort_by_key(|profile| profile.name.to_lowercase());
    profiles
}

/// Записывает профиль через временный файл, чтобы папка никогда не
/// содержала недописанный профиль.
pub fn write(dir: &Path, profile: &ControllerProfile) -> Result<()> {
    fs::create_dir_all(dir)
        .with_context(|| format!("не удалось создать папку профилей: {}", dir.display()))?;
    let path = profile_path(dir, &profile.name);
    let temp = dir.join(format!("{}.{EXTENSION}.tmp", profile.name));
    let stored = ControllerProfile {
        device_guid: String::new(),
        ..profile.clone()
    };
    let mut serialized = serde_json::to_vec_pretty(&stored)?;
    serialized.push(b'\n');
    fs::write(&temp, serialized)
        .with_context(|| format!("не удалось записать профиль: {}", temp.display()))?;
    fs::rename(&temp, &path)
        .with_context(|| format!("не удалось сохранить профиль: {}", path.display()))
}

pub fn remove(dir: &Path, name: &str) -> Result<()> {
    let path = profile_path(dir, name);
    match fs::remove_file(&path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("не удалось удалить профиль: {}", path.display()))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
pub fn test_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("x360ce-test-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> ControllerProfile {
        ControllerProfile::empty(name.to_owned(), "guid".to_owned())
    }

    #[test]
    fn clean_name_keeps_cyrillic_and_replaces_forbidden() {
        assert_eq!(clean_name("DCS: F/A-18"), "DCS- F-A-18");
        assert_eq!(clean_name("  Руль ARTPLAYS. "), "Руль ARTPLAYS");
        assert_eq!(clean_name("..."), "");
    }

    #[test]
    fn folder_round_trip_uses_file_name_as_profile_name() {
        let dir = test_dir("store");
        write(&dir, &named("Профиль 1")).unwrap();
        write(&dir, &named("DCS")).unwrap();
        assert!(!dir.join("DCS.json.tmp").exists());

        // Переименование и удаление файлов в проводнике.
        fs::rename(profile_path(&dir, "DCS"), profile_path(&dir, "DCS F-16")).unwrap();
        fs::write(dir.join("заметка.txt"), "не профиль").unwrap();
        let names: Vec<_> = load_all(&dir, &[]).into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["DCS F-16", "Профиль 1"]);

        remove(&dir, "Профиль 1").unwrap();
        remove(&dir, "Профиль 1").unwrap(); // повторно — не ошибка
        let left = load_all(&dir, &[]);
        assert_eq!(left.len(), 1);
        assert!(left[0].device_guid.is_empty());

        // Испорченный файл не пропадает из списка, пока есть прежняя копия.
        fs::write(profile_path(&dir, "DCS F-16"), "{ недописан").unwrap();
        assert_eq!(load_all(&dir, &left).len(), 1);
        assert!(load_all(&dir, &[]).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
