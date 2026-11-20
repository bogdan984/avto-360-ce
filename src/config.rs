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
    #[serde(default = "default_true")]
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
            forward_only_in_tray: true,
            block_game_bar_controller_button: true,
            last_auto_update_check_unix_seconds: 0,
            last_status: "Готово.".to_owned(),
            joycpl_on_start: false,
            joycpl_device_name: String::new(),
            library: Vec::new(),
        }
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
