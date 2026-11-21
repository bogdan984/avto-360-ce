use crate::model::ControllerProfile;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const DATA_ENV_VAR: &str = "X360CE_APP_DATA_DIR";
const DATA_DIR_NAME: &str = ".x360ce-data";
const CONFIG_FILE_NAME: &str = "config.json";
const BACKUP_FILE_NAME: &str = "config.json.bak";
const SETTINGS_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedState {
    #[serde(default = "default_true")]
    pub emulation_enabled: bool,
    #[serde(default)]
    pub selected_device_guid: String,
    #[serde(default)]
    pub profiles: BTreeMap<String, ControllerProfile>,
    #[serde(default = "default_true")]
    pub auto_tray_enabled: bool,
    #[serde(default = "default_idle_seconds")]
    pub auto_tray_idle_seconds: u64,
    #[serde(default = "default_countdown_seconds")]
    pub auto_tray_countdown_seconds: u64,
    #[serde(default)]
    pub start_in_tray: bool,
    #[serde(default)]
    pub startup_enabled: bool,
    #[serde(default)]
    pub include_prereleases: bool,
    #[serde(default)]
    pub forward_only_in_tray: bool,
    #[serde(default = "default_true")]
    pub block_game_bar_controller_button: bool,
    #[serde(default)]
    pub last_auto_update_check_unix_seconds: u64,
    #[serde(default)]
    pub last_status: String,
    /// Открывать «Свойства» устройства в joy.cpl (за краем экрана) при запуске.
    #[serde(default)]
    pub joycpl_on_start: bool,
    /// Имя устройства в списке joy.cpl; пусто — выбранный контроллер.
    #[serde(default)]
    pub joycpl_device_name: String,
    /// Сохранённые именованные профили (не привязаны к устройству).
    #[serde(default)]
    pub library: Vec<ControllerProfile>,
    /// Виртуальный Xbox-геймпад; выкл — контроллер работает только клавиатурой.
    #[serde(default = "default_true")]
    pub virtual_pad_enabled: bool,
    /// Версия формата настроек (для разовых миграций).
    #[serde(default)]
    pub settings_version: u32,
}

impl Default for SavedState {
    fn default() -> Self {
        Self {
            emulation_enabled: true,
            selected_device_guid: String::new(),
            profiles: BTreeMap::new(),
            auto_tray_enabled: true,
            auto_tray_idle_seconds: default_idle_seconds(),
            auto_tray_countdown_seconds: default_countdown_seconds(),
            start_in_tray: false,
            startup_enabled: false,
            include_prereleases: false,
            forward_only_in_tray: false,
            block_game_bar_controller_button: true,
            last_auto_update_check_unix_seconds: 0,
            last_status: "Готово.".to_owned(),
            joycpl_on_start: false,
            joycpl_device_name: String::new(),
            library: Vec::new(),
            virtual_pad_enabled: true,
            settings_version: SETTINGS_VERSION,
        }
    }
}

impl SavedState {
    /// Записывает текущие назначения в одноимённый профиль списка, чтобы
    /// при переключении профилей каждый показывал свои кнопки.
    pub fn sync_library(&mut self, profile: &ControllerProfile) {
        if let Some(slot) = self.library.iter_mut().find(|saved| saved.name == profile.name) {
            *slot = ControllerProfile {
                device_guid: String::new(),
                ..profile.clone()
            };
        }
    }

    /// Добавляет профиль в список или заменяет одноимённый.
    pub fn upsert_library(&mut self, profile: ControllerProfile) {
        let profile = ControllerProfile {
            device_guid: String::new(),
            ..profile
        };
        match self.library.iter_mut().find(|saved| saved.name == profile.name) {
            Some(slot) => *slot = profile,
            None => self.library.push(profile),
        }
        self.library.sort_by_key(|saved| saved.name.to_lowercase());
    }

    /// Профиль выбранного контроллера (или стандартный, если его ещё нет).
    pub fn active_profile(&self) -> ControllerProfile {
        self.profiles
            .get(&self.selected_device_guid)
            .cloned()
            .unwrap_or_else(|| ControllerProfile::default_for(self.selected_device_guid.clone()))
    }

    /// Делает профиль активным для выбранного контроллера. Прежний активный
    /// профиль остаётся в списке, чтобы его настройки не потерялись.
    pub fn activate(&mut self, mut profile: ControllerProfile) {
        let current = self.active_profile();
        if !self.library.iter().any(|saved| saved.name == current.name) {
            self.upsert_library(current);
        }
        let guid = self.selected_device_guid.clone();
        profile.device_guid = guid.clone();
        self.profiles.insert(guid, profile);
    }

    /// Пустой профиль в списке; покой осей берётся от текущего (это свойство руля).
    pub fn new_empty_profile(&mut self, name: String) -> ControllerProfile {
        let mut profile = ControllerProfile::empty(name, String::new());
        profile.axis_rest = self.active_profile().axis_rest;
        self.upsert_library(profile.clone());
        profile
    }

    /// Свободное имя в списке: `base`, иначе `base (2)`, `base (3)`, …
    pub fn free_library_name(&self, base: &str) -> String {
        let taken = |name: &str| self.library.iter().any(|saved| saved.name == name);
        if !taken(base) {
            return base.to_owned();
        }
        (2..)
            .map(|number| format!("{base} ({number})"))
            .find(|name| !taken(name))
            .unwrap_or_default()
    }
}

fn default_true() -> bool {
    true
}

fn default_idle_seconds() -> u64 {
    60
}

fn default_countdown_seconds() -> u64 {
    10
}

pub fn app_name() -> &'static str {
    "x360ce"
}

pub fn is_dev_build() -> bool {
    cfg!(debug_assertions)
}

pub fn app_version_label() -> String {
    if is_dev_build() {
        env::var("X360CE_DEV_VERSION")
            .unwrap_or_else(|_| format!("v{}-dev", env!("CARGO_PKG_VERSION")))
    } else {
        format!("v{}", env!("CARGO_PKG_VERSION"))
    }
}

pub fn window_title() -> String {
    format!("{} {}", app_name(), app_version_label())
}

pub fn app_data_dir() -> Result<PathBuf> {
    if let Some(path) = env::var_os(DATA_ENV_VAR).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }

    let current_exe = env::current_exe().context("не удалось определить путь к программе")?;
    let exe_dir = current_exe
        .parent()
        .context("не удалось определить папку программы")?;
    Ok(exe_dir.join(DATA_DIR_NAME))
}

pub fn state_path() -> Result<PathBuf> {
    Ok(app_data_dir()?.join(CONFIG_FILE_NAME))
}

pub fn load_state() -> SavedState {
    let Ok(data_dir) = app_data_dir() else {
        return SavedState::default();
    };

    for path in [
        data_dir.join(CONFIG_FILE_NAME),
        data_dir.join(BACKUP_FILE_NAME),
    ] {
        if let Ok(mut state) = read_state_from_path(&path) {
            if state.settings_version < 1 {
                // До 0.4.1 вывод по умолчанию работал только в трее: при открытом
                // окне контроллер «не работал». Теперь работает всегда.
                state.forward_only_in_tray = false;
                state.emulation_enabled = true;
            }
            state.settings_version = SETTINGS_VERSION;
            for profile in state.profiles.values_mut().chain(state.library.iter_mut()) {
                profile.sanitize();
            }
            return state;
        }
    }

    SavedState::default()
}

fn read_state_from_path(path: &Path) -> Result<SavedState> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("не удалось прочитать файл настроек: {}", path.display()))?;
    serde_json::from_str(&contents)
        .with_context(|| format!("не удалось разобрать файл настроек: {}", path.display()))
}

pub fn save_state(state: &SavedState) -> Result<()> {
    let path = state_path()?;
    let parent = path
        .parent()
        .context("не удалось определить папку настроек")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("не удалось создать папку данных: {}", parent.display()))?;

    let temp = parent.join("config.json.tmp");
    let backup = parent.join(BACKUP_FILE_NAME);
    let serialized = serde_json::to_vec_pretty(state)?;

    let mut file = fs::File::create(&temp).with_context(|| {
        format!(
            "не удалось создать временный файл настроек: {}",
            temp.display()
        )
    })?;
    file.write_all(&serialized).with_context(|| {
        format!(
            "не удалось записать временный файл настроек: {}",
            temp.display()
        )
    })?;
    file.write_all(b"\n")?;
    file.sync_all().ok();
    drop(file);

    replace_config_file(&temp, &path, &backup)
}

fn replace_config_file(temp: &Path, path: &Path, backup: &Path) -> Result<()> {
    if !path.exists() {
        return fs::rename(temp, path).with_context(|| {
            format!(
                "не удалось переместить файл настроек: {} -> {}",
                temp.display(),
                path.display()
            )
        });
    }

    if backup.exists() {
        fs::remove_file(backup).ok();
    }
    fs::rename(path, backup).with_context(|| {
        format!(
            "не удалось создать резервную копию настроек: {} -> {}",
            path.display(),
            backup.display()
        )
    })?;

    match fs::rename(temp, path) {
        Ok(()) => {
            fs::remove_file(backup).ok();
            Ok(())
        }
        Err(error) => {
            let _ = fs::rename(backup, path);
            Err(error).with_context(|| {
                format!(
                    "не удалось заменить файл настроек: {} -> {}",
                    temp.display(),
                    path.display()
                )
            })
        }
    }
}

pub fn seconds_since_unix_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{InputBinding, OutputControl};

    fn named(name: &str) -> ControllerProfile {
        ControllerProfile {
            name: name.to_owned(),
            ..ControllerProfile::default_for(String::new())
        }
    }

    #[test]
    fn sync_library_updates_only_profile_with_same_name() {
        let mut state = SavedState {
            library: vec![named("DCS"), named("SnowRunner")],
            ..SavedState::default()
        };
        let mut active = named("DCS");
        active.device_guid = "guid".to_owned();
        active.entry_mut(OutputControl::A).binding = InputBinding::Button { index: 42 };
        state.sync_library(&active);

        let json = |profile: &ControllerProfile| serde_json::to_string(&profile.entries).unwrap();
        let dcs = &state.library[0];
        assert_eq!(json(dcs), json(&active));
        assert!(dcs.device_guid.is_empty());
        assert_ne!(json(&state.library[1]), json(&active));

        state.sync_library(&named("Нет в списке"));
        assert_eq!(state.library.len(), 2);
    }

    /// Сценарий пользователя: профиль 1 настроен, создаём профиль 2 —
    /// в нём ничего из профиля 1; переключения показывают каждый своё.
    #[test]
    fn new_profile_is_empty_and_switching_shows_own_settings() {
        use crate::model::{KeyboardMapping, ModifierConfig, PaddleAxis};
        let json = |profile: &ControllerProfile| {
            let mut profile = profile.clone();
            profile.device_guid.clear();
            serde_json::to_string(&profile).unwrap()
        };
        let mut state = SavedState {
            selected_device_guid: "wheel".to_owned(),
            ..SavedState::default()
        };
        let mut first = ControllerProfile::empty("Профиль 1".to_owned(), String::new());
        first.entry_mut(OutputControl::A).binding = InputBinding::Button { index: 7 };
        first.entry_mut(OutputControl::LeftStickX).binding = InputBinding::AxisPositive { index: 0 };
        first.modifiers.push(ModifierConfig::default());
        first.keyboard.push(KeyboardMapping::default());
        first.paddle_axes.push(PaddleAxis::new(3));
        first.axis_rest = vec![Some(0), Some(-32768)];
        state.profiles.insert("wheel".to_owned(), first.clone());

        // «Новый»: профиль 1 (ещё не в списке) не теряется, профиль 2 пустой.
        let second = state.new_empty_profile("Профиль 2".to_owned());
        state.activate(second);
        let active = state.active_profile();
        assert_eq!(active.name, "Профиль 2");
        assert!(active.entries.iter().all(|entry| entry.binding == InputBinding::None));
        assert!(active.modifiers.is_empty() && active.keyboard.is_empty());
        assert!(active.paddle_axes.is_empty());
        assert_eq!(active.axis_rest, first.axis_rest);
        assert_eq!(state.library.len(), 2);

        // Настраиваем профиль 2 (как profile_changed в окне).
        let mut edited = state.active_profile();
        edited.entry_mut(OutputControl::B).binding = InputBinding::Button { index: 2 };
        state.profiles.insert("wheel".to_owned(), edited.clone());
        state.sync_library(&edited);

        // Обратно на профиль 1 — только его настройки.
        let saved_first = state.library.iter().find(|p| p.name == "Профиль 1").cloned().unwrap();
        state.activate(saved_first);
        assert_eq!(json(&state.active_profile()), json(&first));

        // Снова профиль 2 — его настройки, без кнопок профиля 1.
        let saved_second = state.library.iter().find(|p| p.name == "Профиль 2").cloned().unwrap();
        state.activate(saved_second);
        let active = state.active_profile();
        assert_eq!(json(&active), json(&edited));
        assert_eq!(active.entry(OutputControl::A).unwrap().binding, InputBinding::None);
        assert!(active.modifiers.is_empty());

        // Третий профиль тоже пустой, имена не путаются.
        let third = state.new_empty_profile(state.free_library_name("Новый профиль"));
        assert_eq!(third.name, "Новый профиль");
        state.activate(third);
        assert!(state.active_profile().entries.iter().all(|e| e.binding == InputBinding::None));
        assert_eq!(state.library.len(), 3);
    }

    #[test]
    fn free_library_name_keeps_given_name_when_free() {
        let state = SavedState {
            library: vec![named("DCS"), named("DCS (2)")],
            ..SavedState::default()
        };
        assert_eq!(state.free_library_name("Руль"), "Руль");
        assert_eq!(state.free_library_name("DCS"), "DCS (3)");
    }
}
