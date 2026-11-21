use crate::{model::ControllerProfile, profile_store};
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
const PROFILES_DIR_NAME: &str = "profiles";
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
    /// Именованные профили (не привязаны к устройству). Хранятся файлами в
    /// папке `profiles`; в config.json только читаются (перенос из ≤0.5.2).
    #[serde(default, skip_serializing)]
    pub library: Vec<ControllerProfile>,
    /// Папка профилей; None — только в памяти (тесты).
    #[serde(skip)]
    pub profiles_dir: Option<PathBuf>,
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
            profiles_dir: None,
            virtual_pad_enabled: true,
            settings_version: SETTINGS_VERSION,
        }
    }
}

impl SavedState {
    fn store(&self, profile: &ControllerProfile) {
        if let Some(dir) = &self.profiles_dir {
            let _ = profile_store::write(dir, profile);
        }
    }

    fn unstore(&self, name: &str) {
        if let Some(dir) = &self.profiles_dir {
            let _ = profile_store::remove(dir, name);
        }
    }

    /// Есть ли в списке профиль с таким именем (без учёта регистра, как в Windows).
    pub fn has_profile(&self, name: &str) -> bool {
        self.library.iter().any(|saved| profile_store::same_name(&saved.name, name))
    }

    /// Записывает текущие назначения в одноимённый профиль списка, чтобы
    /// при переключении профилей каждый показывал свои кнопки.
    pub fn sync_library(&mut self, profile: &ControllerProfile) {
        let Some(index) = self.library.iter().position(|saved| saved.name == profile.name) else {
            return;
        };
        let updated = ControllerProfile {
            device_guid: String::new(),
            ..profile.clone()
        };
        let json = |profile: &ControllerProfile| serde_json::to_string(profile).ok();
        if json(&self.library[index]) != json(&updated) {
            self.store(&updated);
            self.library[index] = updated;
        }
    }

    /// Свободное имя в списке: `base`, иначе `base (2)`, `base (3)`, …
    pub fn free_library_name(&self, base: &str) -> String {
        if !self.has_profile(base) {
            return base.to_owned();
        }
        (2..)
            .map(|number| format!("{base} ({number})"))
            .find(|name| !self.has_profile(name))
            .unwrap_or_default()
    }

    /// Добавляет профиль в список или заменяет одноимённый.
    pub fn upsert_library(&mut self, profile: ControllerProfile) {
        let profile = ControllerProfile {
            device_guid: String::new(),
            ..profile
        };
        self.store(&profile);
        self.library
            .retain(|saved| !profile_store::same_name(&saved.name, &profile.name));
        self.library.push(profile);
        self.library.sort_by_key(|saved| saved.name.to_lowercase());
    }

    /// Профиль выбранного контроллера (или стандартный, если его ещё нет).
    pub fn active_profile(&self) -> ControllerProfile {
        self.profiles
            .get(&self.selected_device_guid)
            .cloned()
            .unwrap_or_else(|| ControllerProfile::default_for(self.selected_device_guid.clone()))
    }

    /// Делает профиль активным для выбранного контроллера.
    pub fn activate(&mut self, mut profile: ControllerProfile) {
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

    /// Первый профиль списка (или новый пустой, если список пуст) — замена
    /// активному профилю, которого больше нет.
    fn fallback_profile(&mut self) -> ControllerProfile {
        match self.library.first() {
            Some(first) => first.clone(),
            None => self.new_empty_profile("Профиль 1".to_owned()),
        }
    }

    /// Удаляет профиль из списка и папки. Если он был активным — включается
    /// другой; возвращает его имя.
    pub fn delete_profile(&mut self, name: &str) -> Option<String> {
        self.library.retain(|saved| saved.name != name);
        self.unstore(name);
        if self.active_profile().name != name {
            return None;
        }
        let next = self.fallback_profile();
        let next_name = next.name.clone();
        self.activate(next);
        Some(next_name)
    }

    /// Переименовывает профиль в списке, в папке и у контроллеров.
    pub fn rename_profile(&mut self, old: &str, new: &str) {
        if let Some(saved) = self.library.iter_mut().find(|saved| saved.name == old) {
            saved.name = new.to_owned();
            let renamed = saved.clone();
            self.store(&renamed);
            self.unstore(old);
        }
        self.library.sort_by_key(|saved| saved.name.to_lowercase());
        for profile in self.profiles.values_mut().filter(|profile| profile.name == old) {
            profile.name = new.to_owned();
        }
    }

    /// Подключает папку профилей. При первом запуске (папки ещё нет) переносит
    /// туда список из config.json и активный профиль, чтобы ничего не пропало.
    pub fn open_profiles_dir(&mut self, dir: PathBuf) {
        let legacy = std::mem::take(&mut self.library);
        let first_run = !dir.exists();
        self.profiles_dir = Some(dir);
        if first_run {
            let mut active = self.active_profile();
            let active_name = profile_name_or_default(&active.name);
            for profile in self.profiles.values_mut() {
                profile.name = profile_name_or_default(&profile.name);
            }
            for mut profile in legacy {
                profile.name = profile_name_or_default(&profile.name);
                self.upsert_library(profile);
            }
            if !self.has_profile(&active_name) {
                active.name = active_name;
                self.upsert_library(active);
            }
        }
        self.rescan_profiles();
    }

    /// Перечитывает папку профилей (их могли переименовать или удалить в
    /// проводнике). Возвращает сообщение, если активный профиль сменился.
    pub fn rescan_profiles(&mut self) -> Option<String> {
        let dir = self.profiles_dir.clone()?;
        let fresh = profile_store::load_all(&dir, &self.library);
        let added: Vec<String> = fresh
            .iter()
            .filter(|profile| !self.library.iter().any(|saved| saved.name == profile.name))
            .map(|profile| profile.name.clone())
            .collect();
        self.library = fresh;
        let active = self.active_profile();
        if self.library.iter().any(|saved| saved.name == active.name) {
            return None;
        }
        // Активный файл переименовали — следуем за ним; удалили — берём другой.
        if let [renamed] = added.as_slice() {
            let next = self.library.iter().find(|saved| &saved.name == renamed).cloned()?;
            self.activate(next);
            return Some(format!("Профиль «{}» переименован в папке в «{renamed}».", active.name));
        }
        let next = self.fallback_profile();
        let next_name = next.name.clone();
        self.activate(next);
        Some(format!(
            "Профиля «{}» больше нет в папке — включён «{next_name}».",
            active.name
        ))
    }
}

/// Имя профиля, допустимое как имя файла; пустое — «Профиль».
pub fn profile_name_or_default(name: &str) -> String {
    let cleaned = profile_store::clean_name(name);
    if cleaned.is_empty() {
        "Профиль".to_owned()
    } else {
        cleaned
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
            return with_profiles_dir(state);
        }
    }

    with_profiles_dir(SavedState::default())
}

pub fn profiles_dir() -> Result<PathBuf> {
    Ok(app_data_dir()?.join(PROFILES_DIR_NAME))
}

fn with_profiles_dir(mut state: SavedState) -> SavedState {
    if let Ok(dir) = profiles_dir() {
        state.open_profiles_dir(dir);
    }
    state
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
        state.upsert_library(first.clone());

        // «Новый»: профиль 2 пустой, профиль 1 остаётся в списке.
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

    fn names(state: &SavedState) -> Vec<String> {
        state.library.iter().map(|profile| profile.name.clone()).collect()
    }

    /// Ошибка 0.5.2: удалённый активный профиль возвращался при переключении.
    #[test]
    fn deleted_profile_does_not_come_back() {
        let dir = profile_store::test_dir("delete");
        let mut state = SavedState {
            selected_device_guid: "wheel".to_owned(),
            ..SavedState::default()
        };
        state.open_profiles_dir(dir.clone());
        for name in ["Профиль 1", "Профиль 2", "Профиль 3"] {
            let profile = state.new_empty_profile(name.to_owned());
            state.activate(profile);
        }
        state.delete_profile("Профиль по умолчанию");
        let second = state.library.iter().find(|p| p.name == "Профиль 2").cloned().unwrap();
        state.activate(second);

        // Удаляем активный «Профиль 2» — включается другой, файла нет.
        assert_eq!(state.delete_profile("Профиль 2").as_deref(), Some("Профиль 1"));
        assert_eq!(state.active_profile().name, "Профиль 1");
        assert!(!profile_store::profile_path(&dir, "Профиль 2").exists());

        // Переключения туда-обратно и перечитывание папки его не возвращают.
        for name in ["Профиль 3", "Профиль 1", "Профиль 3"] {
            let next = state.library.iter().find(|p| p.name == name).cloned().unwrap();
            state.activate(next);
            let active = state.active_profile();
            state.sync_library(&active);
            assert_eq!(state.rescan_profiles(), None);
        }
        assert_eq!(names(&state), ["Профиль 1", "Профиль 3"]);

        // После перезапуска программы — тоже нет.
        let mut restarted = SavedState {
            selected_device_guid: "wheel".to_owned(),
            profiles: state.profiles.clone(),
            ..SavedState::default()
        };
        restarted.open_profiles_dir(dir.clone());
        assert_eq!(names(&restarted), ["Профиль 1", "Профиль 3"]);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Папка профилей: перенос из config.json, правки в проводнике.
    #[test]
    fn profiles_folder_follows_explorer_changes() {
        let dir = profile_store::test_dir("folder");
        let mut active = ControllerProfile::empty("DCS: F/A-18".to_owned(), "wheel".to_owned());
        active.entry_mut(OutputControl::A).binding = InputBinding::Button { index: 1 };
        let mut state = SavedState {
            selected_device_guid: "wheel".to_owned(),
            profiles: BTreeMap::from([("wheel".to_owned(), active)]),
            library: vec![named("SnowRunner")],
            ..SavedState::default()
        };
        // Первый запуск 0.5.3: список и активный профиль переносятся в файлы.
        state.open_profiles_dir(dir.clone());
        assert_eq!(names(&state), ["DCS- F-A-18", "SnowRunner"]);
        assert_eq!(state.active_profile().name, "DCS- F-A-18");
        assert!(profile_store::profile_path(&dir, "SnowRunner").exists());

        // Правка кнопки сохраняется в файл профиля.
        let mut edited = state.active_profile();
        edited.entry_mut(OutputControl::B).binding = InputBinding::Button { index: 9 };
        state.profiles.insert("wheel".to_owned(), edited.clone());
        state.sync_library(&edited);
        let on_disk = profile_store::load_all(&dir, &[]);
        assert_eq!(on_disk[0].entry(OutputControl::B).unwrap().binding, InputBinding::Button { index: 9 });

        // Переименовали активный файл в проводнике — профиль следует за ним.
        fs::rename(
            profile_store::profile_path(&dir, "DCS- F-A-18"),
            profile_store::profile_path(&dir, "DCS F-18"),
        )
        .unwrap();
        assert!(state.rescan_profiles().is_some());
        assert_eq!(state.active_profile().name, "DCS F-18");
        assert_eq!(names(&state), ["DCS F-18", "SnowRunner"]);

        // Удалили активный файл в проводнике — включается оставшийся.
        fs::remove_file(profile_store::profile_path(&dir, "DCS F-18")).unwrap();
        assert!(state.rescan_profiles().is_some());
        assert_eq!(state.active_profile().name, "SnowRunner");
        assert_eq!(names(&state), ["SnowRunner"]);

        // Удалили всё — создаётся пустой «Профиль 1», а не воскрешается старый.
        fs::remove_file(profile_store::profile_path(&dir, "SnowRunner")).unwrap();
        state.rescan_profiles();
        assert_eq!(names(&state), ["Профиль 1"]);
        assert_eq!(state.active_profile().name, "Профиль 1");

        // Переименование из окна меняет файл.
        state.rename_profile("Профиль 1", "Руль");
        assert!(profile_store::profile_path(&dir, "Руль").exists());
        assert!(!profile_store::profile_path(&dir, "Профиль 1").exists());
        assert_eq!(state.active_profile().name, "Руль");
        let _ = fs::remove_dir_all(&dir);
    }
}
