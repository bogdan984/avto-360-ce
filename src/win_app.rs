use anyhow::{Context as _, Result};
use crossbeam_channel::{unbounded, Receiver};
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Layout, RichText, Sense, Stroke, TextureHandle,
    TextureOptions,
};
use std::{
    env, ptr, thread,
    time::{Duration, Instant},
};
use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon as TrayImage, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, IsIconic, SetForegroundWindow, ShowWindow, SW_HIDE, SW_RESTORE, SW_SHOW,
};

use crate::{
    config::{self, SavedState},
    driver,
    engine::{ControllerEngine, EngineCommand},
    game_bar, joycpl, mapper,
    model::{
        entry_mut_in, inputs_overlap, key_name, same_device, ControllerProfile, InputBinding, KeyOutput,
        KeyboardMapping, MappingEntry, ModifierConfig, ModifierMode, OutputControl, PaddleAxis,
        PressMode, RawState, RuntimeSnapshot, TriggerType, KEYS, MAX_MODIFIERS,
    },
    profile_io,
    single_instance::BuildKind,
    startup, updater,
    wide::str_wide_null,
};

const DEFAULT_WINDOW_WIDTH: f32 = 1320.0;
const DEFAULT_WINDOW_HEIGHT: f32 = 900.0;
const MIN_WINDOW_WIDTH: f32 = 1120.0;
const MIN_WINDOW_HEIGHT: f32 = 780.0;
const VISIBLE_POLL: Duration = Duration::from_millis(16);
const HIDDEN_POLL: Duration = Duration::from_millis(120);
const AUTO_UPDATE_INTERVAL_SECONDS: u64 = 60 * 60;
/// Автозапуск joy.cpl: попыток × пауза ≈ 2 мин (после включения ПК
/// контроллер и список joy.cpl появляются не сразу).
const JOYCPL_AUTOSTART_ATTEMPTS: u32 = 40;
const JOYCPL_MANUAL_ATTEMPTS: u32 = 3;
const JOYCPL_RETRY_PAUSE: Duration = Duration::from_secs(3);

const BACKGROUND: Color32 = Color32::from_rgb(242, 244, 247);
const SURFACE: Color32 = Color32::from_rgb(255, 255, 255);
const SURFACE_MUTED: Color32 = Color32::from_rgb(248, 249, 251);
const BORDER: Color32 = Color32::from_rgb(218, 222, 229);
const TEXT: Color32 = Color32::from_rgb(28, 31, 38);
const MUTED: Color32 = Color32::from_rgb(102, 108, 120);
const ACCENT: Color32 = Color32::from_rgb(52, 111, 224);
const ACCENT_SOFT: Color32 = Color32::from_rgb(224, 234, 255);
const SUCCESS: Color32 = Color32::from_rgb(37, 145, 83);
const WARNING: Color32 = Color32::from_rgb(205, 125, 21);
const DANGER: Color32 = Color32::from_rgb(199, 58, 58);
const CONTROLLER: Color32 = Color32::from_rgb(43, 47, 56);

const MENU_OPEN: &str = "open";
const MENU_ENABLED: &str = "enabled";
const MENU_STARTUP: &str = "startup";
const MENU_CHECK: &str = "check";
const MENU_INSTALL: &str = "install";
const MENU_QUIT: &str = "quit";

pub fn started_from_startup() -> bool {
    env::args_os().any(|argument| argument == "--startup")
}

pub fn activate_existing_instance(existing_build: BuildKind) {
    let title = existing_build
        .window_title()
        .unwrap_or_else(config::window_title);
    let title = str_wide_null(&title);

    for _ in 0..20 {
        let hwnd = unsafe { FindWindowW(ptr::null(), title.as_ptr()) };
        if !hwnd.is_null() {
            unsafe {
                ShowWindow(
                    hwnd,
                    if IsIconic(hwnd) != 0 {
                        SW_RESTORE
                    } else {
                        SW_SHOW
                    },
                );
                SetForegroundWindow(hwnd);
            }
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

pub fn run() -> Result<()> {
    let icon = load_window_icon(include_bytes!("../assets/x360ce.png"))?;
    let state = config::load_state();
    let visible = config::is_dev_build() || (!started_from_startup() && !state.start_in_tray);
    let viewport = egui::ViewportBuilder::default()
        .with_title(config::window_title())
        .with_inner_size([DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT])
        .with_min_inner_size([MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT])
        .with_resizable(true)
        .with_visible(visible)
        .with_icon(icon);

    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        multisampling: 0,
        centered: true,
        ..Default::default()
    };

    eframe::run_native(
        config::app_name(),
        options,
        Box::new(move |creation_context| {
            Ok(Box::new(X360ceApp::new(creation_context, state, visible)))
        }),
    )
    .map_err(|error| anyhow::anyhow!("не удалось запустить интерфейс x360ce: {error}"))
}

struct TrayState {
    icon: TrayIcon,
    enabled: CheckMenuItem,
    startup: CheckMenuItem,
    install: MenuItem,
}

impl TrayState {
    fn new(state: &SavedState, update_installable: bool) -> Result<Self> {
        let menu = Menu::new();
        let open = MenuItem::with_id(MENU_OPEN, "Открыть x360ce", true, None);
        let enabled = CheckMenuItem::with_id(
            MENU_ENABLED,
            "Виртуальный контроллер включён",
            true,
            state.emulation_enabled,
            None,
        );
        let startup = CheckMenuItem::with_id(
            MENU_STARTUP,
            "Запускать вместе с Windows",
            !config::is_dev_build(),
            state.startup_enabled && !config::is_dev_build(),
            None,
        );
        let check = MenuItem::with_id(
            MENU_CHECK,
            "Проверить обновления",
            !config::is_dev_build(),
            None,
        );
        let install = MenuItem::with_id(
            MENU_INSTALL,
            "Установить обновление",
            update_installable && !config::is_dev_build(),
            None,
        );
        let quit = MenuItem::with_id(MENU_QUIT, "Выйти из x360ce", true, None);
        let separator = PredefinedMenuItem::separator();
        let separator_updates = PredefinedMenuItem::separator();
        let separator_quit = PredefinedMenuItem::separator();
        menu.append_items(&[
            &open,
            &separator,
            &enabled,
            &startup,
            &separator_updates,
            &check,
            &install,
            &separator_quit,
            &quit,
        ])?;

        let icon = TrayIconBuilder::new()
            .with_id("x360ce")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_menu_on_right_click(true)
            .with_icon(load_tray_image(include_bytes!(
                "../assets/x360ce-tray.png"
            ))?)
            .with_tooltip(tray_tooltip(state, false))
            .build()?;

        Ok(Self {
            icon,
            enabled,
            startup,
            install,
        })
    }

    fn sync(&self, state: &SavedState, virtual_connected: bool, update_installable: bool) {
        self.enabled.set_checked(state.emulation_enabled);
        self.startup
            .set_checked(state.startup_enabled && !config::is_dev_build());
        self.install
            .set_enabled(update_installable && !config::is_dev_build());
        let _ = self
            .icon
            .set_tooltip(Some(tray_tooltip(state, virtual_connected)));
    }
}

struct X360ceApp {
    state: SavedState,
    engine: ControllerEngine,
    runtime: RuntimeSnapshot,
    tray: Option<TrayState>,
    status: String,
    controller_texture: Option<TextureHandle>,
    selected_control: OutputControl,
    selected_axis_negative: Option<bool>,
    learning_control: Option<OutputControl>,
    learning_output_negative: bool,
    learn_baseline: RawState,
    window_visible: bool,
    update_rx: Option<Receiver<anyhow::Result<updater::UpdateCheck>>>,
    last_update_check: Option<updater::UpdateCheck>,
    /// Редактируемый слой: None — основной, Some(n) — модификатор n.
    edit_layer: Option<usize>,
    learning_modifier: Option<usize>,
    learning_keyboard_input: Option<usize>,
    learning_key: Option<usize>,
    joycpl_pid: Option<u32>,
    joycpl_rx: Option<Receiver<Result<u32, String>>>,
    /// Поле «Имя» в блоке профилей.
    profile_name_edit: String,
    /// Автоопределение покоя осей: последние значения и с какого момента не менялись.
    rest_probe: Option<(Vec<i16>, Instant)>,
    /// После подключения оси уже менялись (контроллер прислал данные).
    rest_activity: bool,
}

impl X360ceApp {
    fn new(
        creation_context: &eframe::CreationContext<'_>,
        mut state: SavedState,
        window_visible: bool,
    ) -> Self {
        configure_egui(&creation_context.egui_ctx);

        if let Ok(enabled) = startup::is_enabled() {
            state.startup_enabled = enabled;
        }

        let initial_profile = if state.selected_device_guid.is_empty() {
            ControllerProfile::default_for(String::new())
        } else {
            state
                .profiles
                .get(&state.selected_device_guid)
                .cloned()
                .unwrap_or_else(|| {
                    ControllerProfile::default_for(state.selected_device_guid.clone())
                })
        };
        let profile_name_edit = initial_profile.name.clone();
        // Guide output is intentionally always blocked. Keep persisted state
        // aligned with actual behavior and Windows registry policy.
        state.block_game_bar_controller_button = true;

        let forwarding_active = !window_visible || !state.forward_only_in_tray;
        let engine = ControllerEngine::start(
            state.selected_device_guid.clone(),
            initial_profile,
            state.emulation_enabled,
            forwarding_active,
            state.virtual_pad_enabled,
        );
        let game_bar_result = game_bar::install_hard_block();
        let mut status = if state.last_status.is_empty() {
            "Готово.".to_owned()
        } else {
            state.last_status.clone()
        };
        if let Err(error) = game_bar_result {
            status = format!("Не удалось обновить блокировку Xbox Game Bar: {error}");
        }
        let tray = TrayState::new(&state, false).ok();
        let controller_texture = load_controller_texture(&creation_context.egui_ctx);

        // Автозапуск joy.cpl — в своём потоке, а не в цикле отрисовки:
        // при запуске в трей скрытое окно не перерисовывается и joy.cpl не открывался.
        let snapshot = engine.snapshot_handle();
        let manual_name = state.joycpl_device_name.trim().to_owned();
        let selected_guid = state.selected_device_guid.clone();
        let joycpl_rx = spawn_joycpl(None, JOYCPL_AUTOSTART_ATTEMPTS, move || {
            if !manual_name.is_empty() {
                return Some(manual_name.clone());
            }
            let snapshot = snapshot.read().ok()?;
            snapshot
                .devices
                .iter()
                .find(|device| same_device(&device.guid, &selected_guid))
                .or_else(|| snapshot.devices.first())
                .map(|device| device.name.clone())
        });
        let status = format!("{status} Открываю «Игровые устройства» (joy.cpl)…");

        Self {
            state,
            engine,
            runtime: RuntimeSnapshot::default(),
            tray,
            status,
            controller_texture,
            selected_control: OutputControl::A,
            selected_axis_negative: None,
            learning_control: None,
            learning_output_negative: false,
            learn_baseline: RawState::default(),
            window_visible,
            update_rx: None,
            last_update_check: None,
            edit_layer: None,
            learning_modifier: None,
            learning_keyboard_input: None,
            learning_key: None,
            joycpl_pid: None,
            joycpl_rx: Some(joycpl_rx),
            profile_name_edit,
            rest_probe: None,
            rest_activity: false,
        }
    }

    /// Профиль, в котором `entries` — назначения редактируемого слоя.
    fn edit_view(&self) -> ControllerProfile {
        let mut profile = self.current_profile();
        if let Some(layer) = self.edit_layer {
            profile.entries = profile.layer_entries(Some(layer)).to_vec();
        }
        profile
    }

    fn edit_entry_mut(&mut self, control: OutputControl) -> &mut MappingEntry {
        let layer = self.edit_layer;
        let profile = self.current_profile_mut();
        match layer.filter(|layer| *layer < profile.modifiers.len()) {
            Some(layer) => entry_mut_in(&mut profile.modifiers[layer].entries, control),
            None => profile.entry_mut(control),
        }
    }

    fn layer_name(layer: Option<usize>) -> String {
        match layer {
            None => "Основной".to_owned(),
            Some(index) => format!("Модификатор {}", index + 1),
        }
    }

    fn profile_changed(&mut self) {
        self.push_profile();
        self.save();
    }

    fn cancel_learning(&mut self) {
        self.learning_control = None;
        self.learning_output_negative = false;
        self.learning_modifier = None;
        self.learning_keyboard_input = None;
        self.learning_key = None;
    }

    fn current_profile(&self) -> ControllerProfile {
        let guid = self.state.selected_device_guid.clone();
        self.state
            .profiles
            .get(&guid)
            .cloned()
            .unwrap_or_else(|| ControllerProfile::default_for(guid))
    }

    fn current_profile_mut(&mut self) -> &mut ControllerProfile {
        let guid = self.state.selected_device_guid.clone();
        self.state
            .profiles
            .entry(guid.clone())
            .or_insert_with(|| ControllerProfile::default_for(guid))
    }

    fn push_profile(&self) {
        self.engine
            .send(EngineCommand::SetProfile(self.current_profile()));
    }

    fn save(&mut self) {
        self.state.last_status = self.status.clone();
        if let Err(error) = config::save_state(&self.state) {
            self.status = format!("Не удалось сохранить: {error}");
        }
    }

    fn sync_tray(&self) {
        if let Some(tray) = &self.tray {
            tray.sync(
                &self.state,
                self.runtime.virtual_connected,
                self.last_update_check
                    .as_ref()
                    .map(|check| check.is_update_available && check.asset_download_url.is_some())
                    .unwrap_or(false),
            );
        }
    }

    fn select_device(&mut self, guid: String) {
        if self.state.selected_device_guid == guid {
            return;
        }
        self.state.selected_device_guid = guid.clone();
        self.profile_name_edit = self.current_profile_mut().name.clone();
        self.rest_probe = None;
        self.rest_activity = false;
        self.engine.send(EngineCommand::SelectDevice(guid));
        self.push_profile();
        self.status = "Контроллер выбран.".to_owned();
        self.save();
    }

    fn set_emulation_enabled(&mut self, enabled: bool) {
        self.state.emulation_enabled = enabled;
        self.engine.send(EngineCommand::SetEnabled(enabled));
        self.sync_forwarding_mode();
        self.status = if enabled {
            "Контроллер включён.".to_owned()
        } else {
            "Контроллер выключен: ни клавиши, ни Xbox-геймпад не отправляются.".to_owned()
        };
        self.save();
        self.sync_tray();
    }

    fn sync_forwarding_mode(&self) {
        let forwarding_active = !self.window_visible || !self.state.forward_only_in_tray;
        self.engine
            .send(EngineCommand::SetForwardingActive(forwarding_active));
    }

    fn show_window(&mut self, ctx: &egui::Context) {
        self.window_visible = true;
        self.sync_forwarding_mode();
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn hide_window(&mut self, ctx: &egui::Context) {
        self.window_visible = false;
        self.sync_forwarding_mode();
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }

    fn poll_learning(&mut self) {
        if self.learning_modifier.is_none()
            && self.learning_keyboard_input.is_none()
            && self.learning_control.is_none()
        {
            return;
        }
        let Some(binding) = mapper::detect_binding(&self.learn_baseline, &self.runtime.raw_state)
        else {
            return;
        };
        if let InputBinding::AxisPositive { index } | InputBinding::AxisNegative { index } = binding {
            // Покой оси = значение в момент нажатия «Назначить» (педаль отпущена).
            let baseline = self.learn_baseline.axes.clone();
            self.current_profile_mut()
                .remember_rest(&baseline, Some(index as usize));
        }

        if let Some(index) = self.learning_modifier.take() {
            if let Some(modifier) = self.current_profile_mut().modifiers.get_mut(index) {
                modifier.binding = binding;
            }
            self.status = format!("Модификатор {}: {}.", index + 1, binding.label());
            self.profile_changed();
            return;
        }
        if let Some(index) = self.learning_keyboard_input.take() {
            let mut layer = None;
            if let Some(mapping) = self.current_profile_mut().keyboard.get_mut(index) {
                mapping.input = binding;
                layer = mapping.modifier;
            }
            let released = self.release_conflicts(layer, binding, false, None, Some(index));
            self.status = format!(
                "Клавиатурное назначение {}: {}.{released}",
                index + 1,
                binding.label()
            );
            self.profile_changed();
            return;
        }
        let Some(control) = self.learning_control else {
            return;
        };

        let centered_axis = if control.is_trigger() {
            match binding {
                InputBinding::AxisPositive { index } | InputBinding::AxisNegative { index } => {
                    self.learn_baseline
                        .axes
                        .get(index as usize)
                        .copied()
                        .unwrap_or_default()
                        .unsigned_abs()
                        < 8_000
                }
                _ => false,
            }
        } else {
            false
        };
        let output_negative = self.learning_output_negative;
        let full = {
            let mapping = self.edit_entry_mut(control);
            mapping.binding = binding;
            mapping.centered_axis = centered_axis;
            if control.is_axis() {
                mapping.invert = output_negative;
            } else if control.is_trigger() {
                mapping.invert = false;
            }
            mapping.uses_full_axis()
        };
        let released = self.release_conflicts(self.edit_layer, binding, full, Some(control), None);

        self.learning_control = None;
        self.learning_output_negative = false;
        self.status = format!("{}: {}.{released}", control.label(), binding.label());
        self.push_profile();
        self.save();
    }

    fn begin_learning(&mut self, control: OutputControl) {
        self.begin_learning_with_direction(control, false);
    }

    fn begin_learning_with_direction(&mut self, control: OutputControl, output_negative: bool) {
        self.cancel_learning();
        if self.edit_layer.is_some() && control.is_axis() {
            self.selected_control = control;
            self.status = "Стики назначаются только в основном слое.".to_owned();
            return;
        }
        self.selected_control = control;
        self.selected_axis_negative = control.is_axis().then_some(output_negative);
        self.learning_control = Some(control);
        self.learning_output_negative = output_negative;
        self.learn_baseline = self.runtime.raw_state.clone();
        let direction = if control.is_axis() {
            if output_negative {
                "отрицательное направление"
            } else {
                "положительное направление"
            }
        } else {
            "кнопку или ось"
        };
        self.status = format!(
            "Нажмите или отклоните {direction} для «{}».",
            control.label()
        );
    }

    fn begin_learning_input(&mut self, modifier: Option<usize>, keyboard: Option<usize>) {
        self.cancel_learning();
        self.learning_modifier = modifier;
        self.learning_keyboard_input = keyboard;
        self.learn_baseline = self.runtime.raw_state.clone();
        self.status = "Нажмите кнопку на контроллере (Esc — отмена).".to_owned();
    }

    fn begin_learning_key(&mut self, index: usize) {
        self.cancel_learning();
        self.learning_key = Some(index);
        self.status = "Нажмите клавишу на клавиатуре (Esc — отмена).".to_owned();
    }

    /// Захват клавиши клавиатуры для назначения (режим «Захват клавиши»).
    fn poll_key_capture(&mut self, ctx: &egui::Context) {
        let Some(index) = self.learning_key else {
            return;
        };
        let pressed = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Key {
                    key, pressed: true, ..
                } if *key != egui::Key::Escape => egui_key_to_vk(*key),
                _ => None,
            })
        });
        let Some(vk) = pressed else {
            return;
        };
        if let Some(mapping) = self.current_profile_mut().keyboard.get_mut(index) {
            mapping.key.vk = vk;
        }
        self.learning_key = None;
        self.status = format!("Клавиша: {}.", key_name(vk));
        self.profile_changed();
    }

    fn start_joycpl(&mut self) {
        if self.joycpl_rx.is_some() {
            return;
        }
        let name = if self.state.joycpl_device_name.trim().is_empty() {
            self.runtime
                .devices
                .iter()
                .find(|device| device.guid == self.state.selected_device_guid)
                .or_else(|| self.runtime.devices.first())
                .map(|device| device.name.clone())
        } else {
            Some(self.state.joycpl_device_name.clone())
        };
        let previous = self.joycpl_pid.take();
        self.joycpl_rx = Some(spawn_joycpl(previous, JOYCPL_MANUAL_ATTEMPTS, move || {
            name.clone()
        }));
        self.status = "Открываю «Игровые устройства» (joy.cpl)…".to_owned();
    }

    fn close_joycpl(&mut self) {
        if let Some(pid) = self.joycpl_pid.take() {
            thread::spawn(move || joycpl::close(pid));
            self.status = "Свойства устройства (joy.cpl) закрыты.".to_owned();
        }
    }

    fn poll_joycpl(&mut self) {
        let Some(result) = self.joycpl_rx.as_ref().and_then(|rx| rx.try_recv().ok()) else {
            return;
        };
        self.joycpl_rx = None;
        match result {
            Ok(pid) => {
                self.joycpl_pid = Some(pid);
                self.status = "Свойства устройства открыты (окно за краем экрана).".to_owned();
            }
            Err(error) => self.status = format!("joy.cpl: {error}"),
        }
    }

    /// Полный выход: окно прячется сразу, затем закрываются joy.cpl, движок
    /// (отпускает клавиши) и значок в трее. Процесс завершается здесь же —
    /// не зависим от того, обработает ли eframe закрытие скрытого окна.
    fn quit(&mut self) {
        let title = str_wide_null(&config::window_title());
        unsafe {
            let hwnd = FindWindowW(ptr::null(), title.as_ptr());
            if !hwnd.is_null() {
                ShowWindow(hwnd, SW_HIDE);
            }
        }
        self.save();
        self.tray = None;
        self.engine.stop();
        if let Some(pid) = self.joycpl_pid.take() {
            joycpl::close(pid);
        }
        joycpl::close_away();
        std::process::exit(0);
    }

    fn clear_mapping(&mut self, control: OutputControl) {
        self.edit_entry_mut(control).binding = InputBinding::None;
        self.learning_control = None;
        self.learning_output_negative = false;
        self.selected_axis_negative = None;
        self.status = format!("Назначение «{}» очищено.", control.label());
        self.push_profile();
        self.save();
    }

    fn set_axis_binding_direction(&mut self, control: OutputControl, positive: bool) {
        let changed = {
            let mapping = self.edit_entry_mut(control);
            let next = match mapping.binding {
                InputBinding::AxisPositive { index } | InputBinding::AxisNegative { index } => {
                    if positive {
                        InputBinding::AxisPositive { index }
                    } else {
                        InputBinding::AxisNegative { index }
                    }
                }
                _ => return,
            };
            if mapping.binding == next {
                false
            } else {
                mapping.binding = next;
                true
            }
        };
        if changed {
            self.status = format!(
                "Направление источника для «{}»: {}.",
                control.label(),
                if positive { "+" } else { "-" }
            );
            self.push_profile();
            self.save();
        }
    }

    /// Снимает назначения слоя, которые срабатывают от того же физического
    /// входа (Xbox и клавиши), кроме `skip_*`. Возвращает текст для статуса.
    fn release_conflicts(
        &mut self,
        layer: Option<usize>,
        binding: InputBinding,
        full: bool,
        skip_control: Option<OutputControl>,
        skip_key: Option<usize>,
    ) -> String {
        let profile = self.current_profile();
        let (controls, keys) = conflicts_for(&profile, layer, binding, full, skip_control, skip_key);
        if controls.is_empty() && keys.is_empty() {
            return String::new();
        }
        let names = conflict_names(&profile, &controls, &keys);
        let profile = self.current_profile_mut();
        let entries = match layer.filter(|layer| *layer < profile.modifiers.len()) {
            Some(layer) => &mut profile.modifiers[layer].entries,
            None => &mut profile.entries,
        };
        for control in controls {
            entry_mut_in(entries, control).binding = InputBinding::None;
        }
        for index in keys {
            profile.keyboard[index].input = InputBinding::None;
        }
        format!(" Снято с конфликтующих: {names}.")
    }

    /// Запомнить текущее положение всех осей как покой (педали отпущены).
    fn calibrate_rest(&mut self) {
        let axes = self.runtime.raw_state.axes.clone();
        if axes.is_empty() {
            self.status = "Контроллер не найден — нечего запоминать.".to_owned();
            return;
        }
        self.current_profile_mut().remember_rest(&axes, None);
        self.status = format!("Покой осей запомнен ({} осей).", axes.len());
        self.profile_changed();
    }

    /// «Руль — джойстик, остальное — клавиатура»: в основном слое остаётся
    /// только левый стик X (поворот руля), прежние назначения — в библиотеку.
    fn apply_wheel_keyboard_preset(&mut self) {
        let mut backup = self.current_profile();
        backup.name = format!("{} (до «руль + клавиатура»)", backup.name);
        let backup_name = backup.name.clone();
        self.upsert_library(backup);
        for entry in &mut self.current_profile_mut().entries {
            if entry.control != OutputControl::LeftStickX {
                entry.binding = InputBinding::None;
            }
        }
        self.edit_layer = None;
        self.cancel_learning();
        self.status = format!(
            "Руль = левый стик X, остальные назначения Xbox сняты — назначьте кнопки и педали \
             на клавиши. Прежние назначения сохранены в профиль «{backup_name}»."
        );
        self.profile_changed();
    }

    fn upsert_library(&mut self, mut profile: ControllerProfile) {
        profile.device_guid.clear();
        match self.state.library.iter_mut().find(|saved| saved.name == profile.name) {
            Some(slot) => *slot = profile,
            None => self.state.library.push(profile),
        }
        self.state
            .library
            .sort_by_key(|saved| saved.name.to_lowercase());
    }

    /// Делает профиль активным для выбранного контроллера.
    fn activate_profile(&mut self, mut profile: ControllerProfile) {
        let guid = self.state.selected_device_guid.clone();
        profile.device_guid = guid.clone();
        self.profile_name_edit = profile.name.clone();
        self.state.profiles.insert(guid, profile);
        self.edit_layer = None;
        self.cancel_learning();
        self.selected_axis_negative = None;
        self.push_profile();
    }

    fn save_profile_to_library(&mut self) {
        let name = self.profile_name_edit.trim().to_owned();
        if name.is_empty() {
            self.status = "Введите имя профиля.".to_owned();
            return;
        }
        let existed = self.state.library.iter().any(|saved| saved.name == name);
        self.current_profile_mut().name = name.clone();
        let profile = self.current_profile();
        self.upsert_library(profile);
        self.profile_name_edit = name.clone();
        self.status = if existed {
            format!("Профиль «{name}» перезаписан.")
        } else {
            format!("Профиль «{name}» сохранён.")
        };
        self.profile_changed();
    }

    fn load_library_profile(&mut self, name: &str) {
        let Some(profile) = self.state.library.iter().find(|saved| saved.name == name).cloned()
        else {
            return;
        };
        self.activate_profile(profile);
        self.status = format!("Загружен профиль «{name}».");
        self.save();
    }

    fn delete_library_profile(&mut self, name: &str) {
        self.state.library.retain(|saved| saved.name != name);
        self.status = format!("Профиль «{name}» удалён из списка (текущие назначения не изменены).");
        self.save();
    }

    fn export_current_mapping(&mut self) {
        match profile_io::export_profile(&self.current_profile()) {
            Ok(Some(path)) => {
                self.status = format!("Профиль экспортирован: {}", path.display());
            }
            Ok(None) => {
                self.status = "Экспорт отменён.".to_owned();
            }
            Err(error) => {
                self.status = format!("Ошибка экспорта: {error}");
            }
        }
    }

    fn import_mapping(&mut self) {
        match profile_io::import_profile() {
            Ok(Some((path, mut profile))) => {
                let base = profile.name.clone();
                let mut number = 2;
                while self.state.library.iter().any(|saved| saved.name == profile.name) {
                    profile.name = format!("{base} ({number})");
                    number += 1;
                }
                self.upsert_library(profile.clone());
                let name = profile.name.clone();
                self.activate_profile(profile);
                self.status = format!("Профиль «{name}» импортирован и включён: {}", path.display());
                self.save();
            }
            Ok(None) => {
                self.status = "Импорт отменён.".to_owned();
            }
            Err(error) => {
                self.status = format!("Ошибка импорта: {error}");
            }
        }
    }

    fn poll_tray(&mut self, ctx: &egui::Context) {
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            match event {
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
                | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } => self.show_window(ctx),
                _ => {}
            }
        }

        while let Ok(event) = MenuEvent::receiver().try_recv() {
            match event.id.as_ref() {
                MENU_OPEN => self.show_window(ctx),
                MENU_ENABLED => self.set_emulation_enabled(!self.state.emulation_enabled),
                MENU_STARTUP => self.toggle_startup(),
                MENU_CHECK => self.start_update_check(),
                MENU_INSTALL => self.install_update(),
                MENU_QUIT => self.quit(),
                _ => {}
            }
        }
    }

    fn toggle_startup(&mut self) {
        if config::is_dev_build() {
            self.status = "Автозапуск недоступен в сборке для разработки.".to_owned();
            return;
        }
        let enabled = !self.state.startup_enabled;
        match startup::set_enabled(enabled) {
            Ok(()) => {
                self.state.startup_enabled = enabled;
                self.status = if enabled {
                    "Автозапуск с Windows включён.".to_owned()
                } else {
                    "Автозапуск с Windows выключен.".to_owned()
                };
                self.save();
                self.sync_tray();
            }
            Err(error) => self.status = format!("Не удалось изменить автозапуск: {error}"),
        }
    }

    fn maybe_start_automatic_update_check(&mut self) {
        if config::is_dev_build() || self.update_rx.is_some() {
            return;
        }
        let now = config::seconds_since_unix_epoch();
        if now.saturating_sub(self.state.last_auto_update_check_unix_seconds)
            >= AUTO_UPDATE_INTERVAL_SECONDS
        {
            self.start_update_check();
        }
    }

    fn start_update_check(&mut self) {
        if config::is_dev_build() || self.update_rx.is_some() {
            return;
        }
        let include_prereleases = self.state.include_prereleases;
        let (sender, receiver) = unbounded();
        thread::spawn(move || {
            let result = updater::check_for_update(include_prereleases);
            let _ = sender.send(result);
        });
        self.update_rx = Some(receiver);
        self.status = "Проверка релизов на GitHub…".to_owned();
    }

    fn poll_update_check(&mut self) {
        let result = self
            .update_rx
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok());
        let Some(result) = result else {
            return;
        };
        self.update_rx = None;
        self.state.last_auto_update_check_unix_seconds = config::seconds_since_unix_epoch();
        match result {
            Ok(check) => {
                self.status = update_status(&check);
                self.last_update_check = Some(check);
            }
            Err(error) => self.status = format!("Ошибка проверки обновлений: {error}"),
        }
        self.save();
        self.sync_tray();
    }

    fn install_update(&mut self) {
        let Some(check) = self.last_update_check.clone() else {
            self.status = "Нет проверенного обновления.".to_owned();
            return;
        };
        if let Err(error) = updater::install_update(&check) {
            self.status = format!("Ошибка установки обновления: {error}");
        }
    }

    /// Если покой каких-то осей не известен — запоминаем его сами, когда оси
    /// 1,5 с неподвижны. До первых данных SDL отдаёт нули — их не берём.
    fn auto_detect_rest(&mut self) {
        let axes = self.runtime.raw_state.axes.clone();
        let learning = self.learning_control.is_some()
            || self.learning_keyboard_input.is_some()
            || self.learning_modifier.is_some();
        let profile = self.current_profile();
        let missing = (0..axes.len())
            .any(|index| profile.axis_rest.get(index).copied().flatten().is_none());
        if axes.is_empty() || learning || !missing {
            self.rest_probe = None;
            return;
        }
        let now = Instant::now();
        let still = |previous: &[i16]| {
            previous.len() == axes.len()
                && previous
                    .iter()
                    .zip(&axes)
                    .all(|(a, b)| (*a as i32 - *b as i32).abs() < 800)
        };
        match &self.rest_probe {
            Some((previous, since)) if still(previous) => {
                let has_data = self.rest_activity || axes.iter().any(|value| *value != 0);
                if has_data && now.duration_since(*since) >= Duration::from_millis(1500) {
                    self.current_profile_mut().fill_missing_rest(&axes);
                    self.rest_probe = None;
                    self.status = "Положение покоя осей определено автоматически.".to_owned();
                    self.profile_changed();
                }
            }
            previous => {
                if previous.is_some() {
                    self.rest_activity = true;
                }
                self.rest_probe = Some((axes, now));
            }
        }
    }

    fn poll_runtime(&mut self) {
        self.runtime = self.engine.snapshot();
        self.auto_detect_rest();
        let selected = self.state.selected_device_guid.clone();
        if self
            .runtime
            .devices
            .iter()
            .any(|device| device.guid == selected)
        {
            return;
        }
        // Сохранённого GUID нет в списке: пусто — берём первый контроллер,
        // иначе ищем тот же контроллер (VID/PID) под новым GUID и переносим профиль.
        let replacement = self
            .runtime
            .devices
            .iter()
            .find(|device| selected.is_empty() || same_device(&device.guid, &selected))
            .map(|device| device.guid.clone());
        if let Some(guid) = replacement {
            if let Some(profile) = self.state.profiles.get(&selected).cloned() {
                self.state
                    .profiles
                    .entry(guid.clone())
                    .or_insert(ControllerProfile {
                        device_guid: guid.clone(),
                        ..profile
                    });
            }
            self.select_device(guid);
        }
    }
}

impl eframe::App for X360ceApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|input| input.viewport().close_requested()) {
            self.quit();
        }

        let learning = self.learning_control.is_some()
            || self.learning_modifier.is_some()
            || self.learning_keyboard_input.is_some()
            || self.learning_key.is_some();
        if learning && ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.cancel_learning();
            self.status = "Захват назначения отменён.".to_owned();
        }

        self.poll_runtime();
        self.poll_tray(ctx);
        self.poll_learning();
        self.poll_key_capture(ctx);
        self.poll_joycpl();
        self.maybe_start_automatic_update_check();
        self.poll_update_check();
        self.sync_tray();

        ctx.request_repaint_after(if self.window_visible {
            VISIBLE_POLL
        } else {
            HIDDEN_POLL
        });
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        ui_root(self, ui, frame);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.quit();
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        BACKGROUND.to_normalized_gamma_f32()
    }
}

impl X360ceApp {
    fn render_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            draw_embedded_logo(ui);
            ui.vertical(|ui| {
                ui.label(RichText::new("x360ce").size(26.0).strong().color(TEXT));
                ui.label(
                    RichText::new("переназначение контроллера и виртуальный Xbox 360")
                        .size(12.0)
                        .color(MUTED),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("В трей").clicked() {
                    self.hide_window(ui.ctx());
                }
                ui.label(RichText::new(config::app_version_label()).color(MUTED));
            });
        });
    }

    fn render_status(&mut self, ui: &mut egui::Ui) {
        let pad = self.state.virtual_pad_enabled;
        let (label, color) = if !self.state.emulation_enabled {
            ("контроллер выключен", MUTED)
        } else if self.window_visible && self.state.forward_only_in_tray {
            ("пауза: окно открыто", WARNING)
        } else if self.runtime.selected_instance_id.is_none() {
            ("контроллер не найден", WARNING)
        } else if !pad {
            ("работает: клавиатура", SUCCESS)
        } else if !self.runtime.driver_installed {
            ("нет драйвера Xbox", WARNING)
        } else if self.runtime.virtual_connected {
            ("работает", SUCCESS)
        } else {
            ("ожидание", WARNING)
        };

        let runtime_error = self.runtime.last_error.clone();
        compact_surface(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                status_chip(ui, label, color, true);
                let mut pad_next = pad;
                if ui
                    .checkbox(&mut pad_next, "Xbox-геймпад")
                    .on_hover_text(
                        "Виртуальный Xbox 360 для руля/стиков/триггеров. Выключите, если \
                         игра нужна только с клавиатуры — тогда она не увидит геймпад.",
                    )
                    .changed()
                {
                    self.state.virtual_pad_enabled = pad_next;
                    self.engine.send(EngineCommand::SetVirtualPad(pad_next));
                    self.status = if pad_next {
                        "Виртуальный Xbox-геймпад включён.".to_owned()
                    } else {
                        "Виртуальный Xbox-геймпад выключен — работают только клавиши.".to_owned()
                    };
                    self.save();
                }
                status_chip(
                    ui,
                    &format!("найдено: {}", self.runtime.devices.len()),
                    SURFACE_MUTED,
                    false,
                );
                if self
                    .last_update_check
                    .as_ref()
                    .map(|check| check.is_update_available)
                    .unwrap_or(false)
                {
                    status_chip(ui, "есть обновление", WARNING, true);
                }

                ui.add_space(6.0);
                ui.label(RichText::new(&self.status).size(11.0).color(MUTED));

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if pad && !self.runtime.driver_installed && ui.button("Установить драйвер").clicked()
                    {
                        match driver::launch_installer_elevated() {
                            Ok(_) => self.status =
                                "Установщик драйвера открыт. После установки перезапустите x360ce."
                                    .to_owned(),
                            Err(error) => {
                                self.status = format!("Ошибка установщика драйвера: {error}")
                            }
                        }
                    }
                    if ui.button("Обновить").clicked() {
                        self.engine.send(EngineCommand::Refresh);
                        self.status = "Обновление списка контроллеров…".to_owned();
                    }
                    let enabled = self.state.emulation_enabled;
                    let toggle = egui::Button::new(
                        RichText::new(if enabled {
                            "Выключить контроллер"
                        } else {
                            "Включить контроллер"
                        })
                        .strong(),
                    )
                    .fill(if enabled { SURFACE_MUTED } else { SUCCESS });
                    if ui.add(toggle).clicked() {
                        if !enabled {
                            // Включили — значит, должно работать сразу, даже с открытым окном.
                            self.state.forward_only_in_tray = false;
                        }
                        self.set_emulation_enabled(!enabled);
                    }
                });
            });

            if let Some(error) = runtime_error {
                ui.add_space(4.0);
                ui.label(RichText::new(error).size(11.0).color(DANGER));
            }
        });
    }

    fn render_profiles(&mut self, ui: &mut egui::Ui) {
        surface(ui, |ui| {
            let active = self.current_profile();
            let saved = self
                .state
                .library
                .iter()
                .find(|saved| saved.name == active.name)
                .map(|saved| same_mappings(saved, &active));
            ui.horizontal(|ui| {
                ui.label(RichText::new("Профили").strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (text, color) = match saved {
                        Some(true) => ("сохранён", SUCCESS),
                        Some(false) => ("есть несохранённые изменения", WARNING),
                        None => ("не сохранён в список", MUTED),
                    };
                    ui.label(RichText::new(text).size(11.0).color(color));
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Имя:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.profile_name_edit)
                        .hint_text("Название профиля")
                        .desired_width(200.0),
                );
                if ui
                    .button("Сохранить")
                    .on_hover_text("Сохранить текущие назначения под этим именем (перезапишет одноимённый)")
                    .clicked()
                {
                    self.save_profile_to_library();
                }
            });
            let names: Vec<String> = self.state.library.iter().map(|saved| saved.name.clone()).collect();
            let mut load = None;
            let mut delete = None;
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt("profile-library")
                    .selected_text(format!("Загрузить ({})", names.len()))
                    .width(170.0)
                    .show_ui(ui, |ui| {
                        if names.is_empty() {
                            ui.label("Список пуст — нажмите «Сохранить»");
                        }
                        for name in &names {
                            if ui.selectable_label(*name == active.name, name).clicked() {
                                load = Some(name.clone());
                            }
                        }
                    });
                if ui.button("Импорт…").on_hover_text("Загрузить профиль из файла .json").clicked() {
                    self.import_mapping();
                }
                if ui.button("Экспорт…").on_hover_text("Сохранить текущий профиль в файл .json").clicked() {
                    self.export_current_mapping();
                }
                let name = self.profile_name_edit.trim().to_owned();
                if ui
                    .add_enabled(names.contains(&name), egui::Button::new("Удалить"))
                    .on_hover_text("Удалить профиль с этим именем из списка")
                    .clicked()
                {
                    delete = Some(name);
                }
            });
            if let Some(name) = load {
                self.load_library_profile(&name);
            }
            if let Some(name) = delete {
                self.delete_library_profile(&name);
            }
        });
    }

    fn render_device_bar(&mut self, ui: &mut egui::Ui) {
        surface(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Физический контроллер").strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("найдено: {}", self.runtime.devices.len()))
                            .size(11.0)
                            .color(MUTED),
                    );
                });
            });
            ui.add_space(6.0);

            let selected_name = self
                .runtime
                .devices
                .iter()
                .find(|device| device.guid == self.state.selected_device_guid)
                .map(|device| ellipsize(&device.name, 34))
                .unwrap_or_else(|| "Контроллер не найден".to_owned());
            let combo_width = (ui.available_width() - 76.0).clamp(160.0, 280.0);
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("controller-selector")
                    .selected_text(selected_name)
                    .width(combo_width)
                    .show_ui(ui, |ui| {
                        let devices = self.runtime.devices.clone();
                        for device in devices {
                            let label = format!(
                                "{} · осей: {} · кнопок: {} · хаток: {}",
                                device.name, device.axes, device.buttons, device.hats
                            );
                            if ui
                                .selectable_label(
                                    self.state.selected_device_guid == device.guid,
                                    label,
                                )
                                .clicked()
                            {
                                self.select_device(device.guid);
                            }
                        }
                    });
                if ui.button("Обновить").clicked() {
                    self.engine.send(EngineCommand::Refresh);
                    self.status = "Обновление списка контроллеров…".to_owned();
                }
            });
        });
    }

    fn render_controller(&mut self, ui: &mut egui::Ui) {
        surface(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("Раскладка виртуального Xbox 360").strong());
                    ui.label(
                        RichText::new(
                            "Щелчок — выбрать · двойной щелчок — назначить · Esc — отмена",
                        )
                        .size(11.0)
                        .color(MUTED),
                    );
                });
                if let Some(control) = self.learning_control {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        status_chip(
                            ui,
                            &format!("назначение: {}", control.label()),
                            ACCENT,
                            true,
                        );
                    });
                }
            });

            ui.add_space(4.0);
            let width = ui.available_width();
            let height = (width * 0.62).clamp(260.0, 340.0);
            let (area, _) = ui.allocate_exact_size(egui::vec2(width, height), Sense::hover());
            // Сверху место под триггеры, картинка вписывается с сохранением пропорций.
            let image_area = egui::Rect::from_min_max(
                egui::pos2(area.left(), area.top() + height * 0.16),
                area.max,
            );
            let image_height = image_area
                .height()
                .min(image_area.width() / CONTROLLER_IMAGE_ASPECT);
            let rect = egui::Rect::from_center_size(
                egui::pos2(
                    image_area.center().x,
                    image_area.bottom() - image_height * 0.5,
                ),
                egui::vec2(image_height * CONTROLLER_IMAGE_ASPECT, image_height),
            );
            let texture_id = self.controller_texture.as_ref().map(|texture| texture.id());
            if let Some(texture_id) = texture_id {
                ui.painter().image(
                    texture_id,
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            } else {
                draw_controller_body(ui, rect);
            }

            let profile = self.edit_view();
            for (control, x, y) in controller_points() {
                let center = egui::pos2(
                    rect.left() + rect.width() * x,
                    rect.top() + rect.height() * y,
                );
                let radius = if control.is_trigger() {
                    13.0
                } else if matches!(
                    control,
                    OutputControl::Start | OutputControl::Back | OutputControl::Guide
                ) {
                    17.0
                } else {
                    11.0
                };
                controller_control(
                    self,
                    ui,
                    &profile,
                    control,
                    center,
                    radius,
                    short_control_label(control),
                );
            }

            draw_stick_controls(
                self,
                ui,
                &profile,
                true,
                egui::pos2(
                    rect.left() + rect.width() * 0.360,
                    rect.top() + rect.height() * 0.471,
                ),
            );
            draw_stick_controls(
                self,
                ui,
                &profile,
                false,
                egui::pos2(
                    rect.left() + rect.width() * 0.642,
                    rect.top() + rect.height() * 0.471,
                ),
            );
        });
    }

    fn render_mapping_editor(&mut self, ui: &mut egui::Ui) {
        surface(ui, |ui| {
            let control = self.selected_control;
            let profile = self.edit_view();
            let selected_entry = profile.entry(control).cloned();
            ui.label(
                RichText::new(format!("Слой: {}", Self::layer_name(self.edit_layer)))
                    .size(11.0)
                    .color(ACCENT),
            );
            let selected_mapped = selected_entry
                .as_ref()
                .map(|entry| entry.binding != InputBinding::None)
                .unwrap_or(false);

            let title = ui.add(
                egui::Label::new(RichText::new(control.label()).size(18.0).strong())
                    .sense(Sense::click()),
            );
            if title.double_clicked() {
                self.begin_learning(control);
            }
            ui.label(
                RichText::new(entry_label_for(&profile, control)).color(if selected_mapped {
                    MUTED
                } else {
                    WARNING
                }),
            );

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button("Назначить").clicked() {
                    self.begin_learning(control);
                }
                if ui
                    .add_enabled(selected_mapped, egui::Button::new("Очистить"))
                    .clicked()
                {
                    self.clear_mapping(control);
                }
            });
            ui.label(
                RichText::new(format!(
                    "Сейчас нажато: {}",
                    primary_live_input_label(&self.runtime.raw_state)
                ))
                .size(11.0)
                .color(MUTED),
            );

            if let Some(entry) = selected_entry
                .as_ref()
                .filter(|entry| entry.binding != InputBinding::None)
            {
                let full_profile = self.current_profile();
                let (controls, keys) = conflicts_for(
                    &full_profile,
                    self.edit_layer,
                    entry.binding,
                    entry.uses_full_axis(),
                    Some(control),
                    None,
                );
                if !controls.is_empty() || !keys.is_empty() {
                    let names = conflict_names(&full_profile, &controls, &keys);
                    let note = if keys.is_empty() {
                        "оба сработают одновременно"
                    } else {
                        "клавиша главнее — как Xbox этот вход не сработает"
                    };
                    let mut release = false;
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(format!("! Тот же вход у: {names} ({note})."))
                                .size(11.0)
                                .color(WARNING),
                        );
                        release = ui.small_button("Снять с них").clicked();
                    });
                    if release {
                        let released = self.release_conflicts(
                            self.edit_layer,
                            entry.binding,
                            entry.uses_full_axis(),
                            Some(control),
                            None,
                        );
                        self.status = format!("{}:{released}", control.label());
                        self.profile_changed();
                    }
                }
            }

            if !control.is_axis() && (self.edit_layer.is_none() || selected_mapped) {
                let current = selected_entry
                    .as_ref()
                    .map(|entry| entry.trigger)
                    .unwrap_or_default();
                let mut next = current;
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Тип нажатия").size(11.0).color(MUTED));
                    egui::ComboBox::from_id_salt("trigger-type")
                        .selected_text(current.label())
                        .show_ui(ui, |ui| {
                            for trigger in TriggerType::ALL {
                                ui.selectable_value(&mut next, trigger, trigger.label());
                            }
                        });
                });
                if next != current {
                    self.edit_entry_mut(control).trigger = next;
                    self.status = format!("{}: тип нажатия «{}».", control.label(), next.label());
                    self.profile_changed();
                }
            }

            if control.is_axis() && self.edit_layer.is_some() {
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Стики работают только в основном слое.")
                        .size(11.0)
                        .color(WARNING),
                );
            } else if control.is_axis() || control.is_trigger() {
                ui.add_space(8.0);
                let binding = selected_entry
                    .as_ref()
                    .map(|entry| entry.binding)
                    .unwrap_or_default();
                let mut calibrate = false;
                if control.is_trigger() {
                    if let Some(positive) = axis_binding_positive(binding) {
                        ui.label(
                            RichText::new("Срабатывает, когда ось уходит от покоя")
                                .size(11.0)
                                .color(MUTED),
                        );
                        ui.horizontal(|ui| {
                            if ui.selectable_label(!positive, "в минус (−)").clicked() {
                                self.set_axis_binding_direction(control, false);
                            }
                            if ui.selectable_label(positive, "в плюс (+)").clicked() {
                                self.set_axis_binding_direction(control, true);
                            }
                        });
                        calibrate = rest_row(ui, &self.runtime.raw_state, binding);
                    }
                } else {
                    if let Some(positive) = axis_binding_positive(binding) {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Направление источника")
                                    .size(11.0)
                                    .color(MUTED),
                            );
                            if ui.selectable_label(positive, "+").clicked() {
                                self.set_axis_binding_direction(control, true);
                            }
                            if ui.selectable_label(!positive, "−").clicked() {
                                self.set_axis_binding_direction(control, false);
                            }
                        });
                    }
                }

                let mut changed = false;
                {
                    let mapping = self.edit_entry_mut(control);
                    ui.horizontal_wrapped(|ui| {
                        changed |= ui.checkbox(&mut mapping.invert, "Инвертировать").changed();
                    });
                    changed |= ui
                        .add(
                            egui::Slider::new(&mut mapping.deadzone, 0.0..=0.5)
                                .text("Мёртвая зона")
                                .fixed_decimals(2),
                        )
                        .changed();
                    changed |= ui
                        .add(
                            egui::Slider::new(&mut mapping.saturation, 0.5..=1.0)
                                .text("Максимум")
                                .fixed_decimals(2),
                        )
                        .changed();
                }
                if changed {
                    self.push_profile();
                    self.save();
                }
                if calibrate {
                    self.calibrate_rest();
                }
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Назначения").strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new("двойной щелчок — назначить · × — очистить")
                            .size(10.0)
                            .color(MUTED),
                    );
                });
            });
            ui.add_space(4.0);

            let layer = self.edit_layer;
            let controls: Vec<OutputControl> = OutputControl::BUTTONS
                .into_iter()
                .chain(OutputControl::ANALOGS)
                .filter(|control| layer.is_none() || !control.is_axis())
                .collect();
            let split = controls.len().div_ceil(2);
            ui.columns(2, |columns| {
                for (column_index, chunk) in controls.chunks(split).enumerate() {
                    columns[column_index].vertical(|ui| {
                        for item in chunk {
                            let entry = profile.entry(*item);
                            let mapped = entry
                                .map(|entry| entry.binding != InputBinding::None)
                                .unwrap_or(false);
                            let active = entry
                                .map(|entry| {
                                    control_preview_active(*item, entry, &self.runtime.raw_state)
                                })
                                .unwrap_or(false);
                            let label = if !mapped && layer.is_some() {
                                format!("{} · —", compact_control_label(*item))
                            } else if mapped {
                                format!(
                                    "{} · {}",
                                    compact_control_label(*item),
                                    entry_label_for(&profile, *item)
                                )
                            } else {
                                format!("! {} · Не назначено", compact_control_label(*item))
                            };
                            let rich = if active {
                                RichText::new(label).strong().color(SUCCESS)
                            } else if mapped || layer.is_some() {
                                RichText::new(label).size(11.0).color(TEXT)
                            } else {
                                RichText::new(label)
                                    .size(11.0)
                                    .color(Color32::from_rgba_unmultiplied(154, 112, 45, 170))
                            };

                            let mut clear_clicked = false;
                            let response = ui
                                .horizontal(|ui| {
                                    // Подпись фиксированной ширины — все «×» на одной вертикали.
                                    let label_width = (ui.available_width() - 30.0).max(110.0);
                                    let response = ui
                                        .allocate_ui_with_layout(
                                            egui::vec2(label_width, ui.spacing().interact_size.y),
                                            Layout::left_to_right(Align::Center),
                                            |ui| {
                                                ui.set_min_width(label_width);
                                                ui.add(
                                                    egui::Button::selectable(
                                                        self.selected_control == *item,
                                                        rich,
                                                    )
                                                    .truncate(),
                                                )
                                            },
                                        )
                                        .inner;
                                    clear_clicked = ui
                                        .add_enabled(mapped, egui::Button::new("×"))
                                        .on_hover_text("Очистить назначение")
                                        .clicked();
                                    response
                                })
                                .inner;

                            if clear_clicked {
                                self.clear_mapping(*item);
                            } else if response.double_clicked() {
                                self.begin_learning(*item);
                            } else if response.clicked() {
                                self.selected_control = *item;
                                self.selected_axis_negative = None;
                            }
                        }
                    });
                }
            });
        });
    }

    fn render_layers(&mut self, ui: &mut egui::Ui) {
        surface(ui, |ui| {
            ui.label(RichText::new("Модификаторы и слои").strong());
            ui.label(
                RichText::new(
                    "Пока модификатор зажат, кнопки работают по назначениям его слоя, \
                     а их обычное действие блокируется. Модификатором может быть любая кнопка.",
                )
                .size(11.0)
                .color(MUTED),
            );
            ui.add_space(6.0);

            let profile = self.current_profile();
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Редактируемый слой:").size(11.0).color(MUTED));
                let layers = std::iter::once(None).chain((0..profile.modifiers.len()).map(Some));
                for layer in layers {
                    if ui
                        .selectable_label(self.edit_layer == layer, Self::layer_name(layer))
                        .clicked()
                    {
                        self.edit_layer = layer;
                        self.cancel_learning();
                    }
                }
            });
            ui.add_space(4.0);

            let mut remove = None;
            let mut learn = None;
            let mut new_mode = None;
            for (index, modifier) in profile.modifiers.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("Модификатор {}:", index + 1)).strong());
                    let held = mapper::input_pressed(modifier.binding, &self.runtime.raw_state);
                    let text = if self.learning_modifier == Some(index) {
                        RichText::new("нажмите кнопку…").color(ACCENT)
                    } else if modifier.binding == InputBinding::None {
                        RichText::new("не назначен").color(WARNING)
                    } else if held {
                        RichText::new(modifier.binding.label())
                            .strong()
                            .color(SUCCESS)
                    } else {
                        RichText::new(modifier.binding.label()).color(TEXT)
                    };
                    ui.label(text);
                    if ui.button("Назначить").clicked() {
                        learn = Some(index);
                    }
                    let mut mode = modifier.mode;
                    egui::ComboBox::from_id_salt(("modifier-mode", index))
                        .selected_text(mode.label())
                        .width(250.0)
                        .show_ui(ui, |ui| {
                            for item in ModifierMode::ALL {
                                ui.selectable_value(&mut mode, item, item.label());
                            }
                        });
                    if mode != modifier.mode {
                        new_mode = Some((index, mode));
                    }
                    if ui
                        .button("×")
                        .on_hover_text("Удалить модификатор")
                        .clicked()
                    {
                        remove = Some(index);
                    }
                });
            }
            if let Some(index) = learn {
                self.begin_learning_input(Some(index), None);
            }
            if let Some((index, mode)) = new_mode {
                self.current_profile_mut().modifiers[index].mode = mode;
                self.status = format!("Модификатор {}: {}.", index + 1, mode.label());
                self.profile_changed();
            }
            if let Some(index) = remove {
                self.current_profile_mut().remove_modifier(index);
                self.edit_layer = None;
                self.cancel_learning();
                self.status = format!("Модификатор {} удалён.", index + 1);
                self.profile_changed();
            }
            if ui
                .add_enabled(
                    profile.modifiers.len() < MAX_MODIFIERS,
                    egui::Button::new("+ Добавить модификатор"),
                )
                .clicked()
            {
                let index = {
                    let profile = self.current_profile_mut();
                    profile.modifiers.push(ModifierConfig::default());
                    profile.modifiers.len() - 1
                };
                self.profile_changed();
                self.begin_learning_input(Some(index), None);
            }
            ui.label(
                RichText::new(
                    "«Своё действие при одиночном нажатии» — назначения этой кнопки в основном \
                     слое срабатывают коротким нажатием, если её отпустили без комбинации.",
                )
                .size(10.0)
                .color(MUTED),
            );

            ui.add_space(8.0);
            ui.label(RichText::new("Тайминги").strong());
            let mut timings = profile.timings;
            ui.add(
                egui::Slider::new(&mut timings.double_tap_window_ms, 100..=800)
                    .text("Окно двойного нажатия, мс"),
            );
            ui.add(
                egui::Slider::new(&mut timings.hold_threshold_ms, 150..=2000)
                    .text("Порог удержания, мс"),
            );
            if timings != profile.timings {
                self.current_profile_mut().timings = timings;
                self.profile_changed();
            }
            ui.label(
                RichText::new(
                    "«Прямое» срабатывает без задержки. Если на кнопке есть «Двойное» или \
                     «Удержание», «Одиночное» срабатывает после окна ожидания или при отпускании.",
                )
                .size(10.0)
                .color(MUTED),
            );
        });
    }

    fn render_keyboard(&mut self, ui: &mut egui::Ui) {
        surface(ui, |ui| {
            ui.label(RichText::new("Клавиатура").strong());
            ui.label(
                RichText::new(
                    "Кнопки контроллера нажимают клавиши Windows. Вход, назначенный на \
                     клавишу, перестаёт работать как Xbox — конфликтов нет. При включённом \
                     «Работать только в трее» клавиши отправляются, только когда окно \
                     свёрнуто в трей.",
                )
                .size(11.0)
                .color(MUTED),
            );
            ui.add_space(4.0);
            if ui
                .button("Руль — джойстик, остальное — клавиатура")
                .on_hover_text(
                    "Оставить джойстиком только поворот руля (левый стик X), все остальные \
                     назначения Xbox снять. Прежние назначения сохраняются в профиль.",
                )
                .clicked()
            {
                self.apply_wheel_keyboard_preset();
            }
            if ui
                .button("Запомнить покой осей")
                .on_hover_text(
                    "Отпустите педали и лепестки, руль — по центру, и нажмите. Педали \
                     срабатывают по ходу от этого положения (запоминается и само при назначении).",
                )
                .clicked()
            {
                self.calibrate_rest();
            }
            ui.add_space(6.0);

            let profile = self.current_profile();
            let layers: Vec<Option<usize>> = std::iter::once(None)
                .chain((0..profile.modifiers.len()).map(Some))
                .collect();
            let mut updated = None;
            let mut remove = None;
            let mut learn_input = None;
            let mut learn_key = None;
            for (index, mapping) in profile.keyboard.iter().enumerate() {
                let mut next = mapping.clone();
                egui::Frame::new()
                    .fill(SURFACE_MUTED)
                    .corner_radius(8)
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(format!("{}.", index + 1)).strong());
                            let input = if self.learning_keyboard_input == Some(index) {
                                "нажмите кнопку…".to_owned()
                            } else {
                                next.input.label()
                            };
                            let active = mapper::input_pressed(next.input, &self.runtime.raw_state);
                            let input = if active {
                                RichText::new(input).strong().color(SUCCESS)
                            } else {
                                RichText::new(input)
                            };
                            if ui
                                .button(input)
                                .on_hover_text("Назначить кнопку контроллера")
                                .clicked()
                            {
                                learn_input = Some(index);
                            }
                            if let InputBinding::AxisPositive { index: axis }
                            | InputBinding::AxisNegative { index: axis } = next.input
                            {
                                let positive =
                                    matches!(next.input, InputBinding::AxisPositive { .. });
                                if ui
                                    .selectable_label(!positive, "−")
                                    .on_hover_text("Срабатывать, когда ось уходит в минус")
                                    .clicked()
                                {
                                    next.input = InputBinding::AxisNegative { index: axis };
                                }
                                if ui
                                    .selectable_label(positive, "+")
                                    .on_hover_text("Срабатывать, когда ось уходит в плюс")
                                    .clicked()
                                {
                                    next.input = InputBinding::AxisPositive { index: axis };
                                }
                                let travel = mapper::axis_travel(
                                    &self.runtime.raw_state,
                                    axis,
                                    !positive,
                                    0,
                                );
                                ui.label(
                                    RichText::new(format!("ход {:.0}%", travel * 100.0))
                                        .size(10.0)
                                        .color(MUTED),
                                )
                                .on_hover_text("Клавиша нажимается от 40% хода от покоя");
                            }
                            egui::ComboBox::from_id_salt(("key-layer", index))
                                .selected_text(Self::layer_name(next.modifier))
                                .show_ui(ui, |ui| {
                                    for layer in &layers {
                                        ui.selectable_value(
                                            &mut next.modifier,
                                            *layer,
                                            Self::layer_name(*layer),
                                        );
                                    }
                                });
                            egui::ComboBox::from_id_salt(("key-trigger", index))
                                .selected_text(next.trigger.label())
                                .show_ui(ui, |ui| {
                                    for trigger in TriggerType::ALL {
                                        ui.selectable_value(
                                            &mut next.trigger,
                                            trigger,
                                            trigger.label(),
                                        );
                                    }
                                });
                            if ui.button("×").on_hover_text("Удалить назначение").clicked()
                            {
                                remove = Some(index);
                            }
                        });
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Клавиша:");
                            let key = if self.learning_key == Some(index) {
                                "нажмите клавишу…".to_owned()
                            } else {
                                next.key.label()
                            };
                            if ui.button(key).on_hover_text("Захват клавиши").clicked()
                            {
                                learn_key = Some(index);
                            }
                            egui::ComboBox::from_id_salt(("key-list", index))
                                .selected_text("Список")
                                .height(320.0)
                                .show_ui(ui, |ui| {
                                    for (vk, name) in KEYS {
                                        ui.selectable_value(&mut next.key.vk, *vk, *name);
                                    }
                                });
                            ui.checkbox(&mut next.key.ctrl, "Ctrl");
                            ui.checkbox(&mut next.key.shift, "Shift");
                            ui.checkbox(&mut next.key.alt, "Alt");
                            ui.checkbox(&mut next.key.win, "Win");
                            egui::ComboBox::from_id_salt(("key-mode", index))
                                .selected_text(press_mode_label(next.key.mode))
                                .show_ui(ui, |ui| {
                                    for mode in [PressMode::HoldWhilePressed, PressMode::Pulse] {
                                        ui.selectable_value(
                                            &mut next.key.mode,
                                            mode,
                                            press_mode_label(mode),
                                        );
                                    }
                                });
                        });
                        let (controls, _) =
                            conflicts_for(&profile, next.modifier, next.input, false, None, Some(index));
                        if next.key.vk != 0 && !controls.is_empty() {
                            ui.label(
                                RichText::new(format!(
                                    "Как Xbox этот вход отключён: {}",
                                    conflict_names(&profile, &controls, &[])
                                ))
                                .size(10.0)
                                .color(MUTED),
                            );
                        }
                    });
                ui.add_space(4.0);
                if next != *mapping {
                    updated = Some((index, next));
                }
            }

            if let Some((index, mapping)) = updated {
                self.current_profile_mut().keyboard[index] = mapping;
                self.profile_changed();
            }
            if let Some(index) = remove {
                self.current_profile_mut().keyboard.remove(index);
                self.cancel_learning();
                self.status = format!("Клавиатурное назначение {} удалено.", index + 1);
                self.profile_changed();
            }
            if let Some(index) = learn_input {
                self.begin_learning_input(None, Some(index));
            }
            if let Some(index) = learn_key {
                self.begin_learning_key(index);
            }
            if ui.button("+ Добавить клавишу").clicked() {
                let layer = self.edit_layer;
                let index = {
                    let profile = self.current_profile_mut();
                    profile.keyboard.push(KeyboardMapping {
                        modifier: layer,
                        key: KeyOutput::default(),
                        ..Default::default()
                    });
                    profile.keyboard.len() - 1
                };
                self.profile_changed();
                self.begin_learning_input(None, Some(index));
            }
        });
    }

    fn render_paddles(&mut self, ui: &mut egui::Ui) {
        surface(ui, |ui| {
            ui.label(RichText::new("Педали и лепестки на одной оси").strong());
            ui.label(
                RichText::new(
                    "Если лепесток двигает ту же ось, что и педаль: быстрый скачок оси до упора \
                     считается лепестком — это отдельный вход, его можно назначить на любую \
                     кнопку или клавишу. Педаль в это время держит своё значение.",
                )
                .size(11.0)
                .color(MUTED),
            );
            ui.add_space(6.0);

            let profile = self.current_profile();
            let raw = self.runtime.raw_state.clone();
            let mut updated = None;
            let mut remove = None;
            for (index, item) in profile.paddle_axes.iter().enumerate() {
                let mut next = *item;
                let axis_count = raw.axes.len().max(next.axis as usize + 1);
                ui.horizontal_wrapped(|ui| {
                    egui::ComboBox::from_id_salt(("paddle-axis", index))
                        .selected_text(format!("Ось {}", next.axis + 1))
                        .show_ui(ui, |ui| {
                            for axis in 0..axis_count as u32 {
                                ui.selectable_value(
                                    &mut next.axis,
                                    axis,
                                    format!("Ось {}", axis + 1),
                                );
                            }
                        });
                    for negative in [true, false] {
                        let on = raw.paddles.contains(&(next.axis, negative));
                        status_chip(
                            ui,
                            &format!("лепесток {}", if negative { "-" } else { "+" }),
                            if on { SUCCESS } else { SURFACE_MUTED },
                            on,
                        );
                    }
                    if ui.button("×").on_hover_text("Убрать разделение").clicked()
                    {
                        remove = Some(index);
                    }
                });
                ui.add(
                    egui::Slider::new(&mut next.min_jump, 0.15..=0.8)
                        .text("Минимальный скачок, доля хода")
                        .fixed_decimals(2),
                );
                ui.add(egui::Slider::new(&mut next.window_ms, 10..=60).text("Окно скачка, мс"));
                if next != *item {
                    updated = Some((index, next));
                }
            }
            if let Some((index, item)) = updated {
                self.current_profile_mut().paddle_axes[index] = item;
                self.profile_changed();
            }
            if let Some(index) = remove {
                self.current_profile_mut().paddle_axes.remove(index);
                self.profile_changed();
            }
            if ui.button("+ Разделить педаль и лепестки на оси").clicked()
            {
                // Предлагаем ось, которая сейчас сильнее всего отклонена.
                let axis = raw
                    .axes
                    .iter()
                    .enumerate()
                    .max_by_key(|(_, value)| value.unsigned_abs())
                    .map(|(index, _)| index as u32)
                    .unwrap_or_default();
                self.current_profile_mut()
                    .paddle_axes
                    .push(PaddleAxis::new(axis));
                self.profile_changed();
            }
            ui.label(
                RichText::new(
                    "Проверка: нажмите лепесток — загорается «лепесток»; плавно и резко \
                     нажмите педаль — не должен. Если педаль ловится как лепесток, увеличьте \
                     «Минимальный скачок» или уменьшите «Окно». Лепесток, нажатый при \
                     полностью выжатой педали, распознать нельзя: ось уже у упора.",
                )
                .size(10.0)
                .color(MUTED),
            );
        });
    }

    fn render_raw_inputs(&mut self, ui: &mut egui::Ui) {
        surface(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Физический ввод в реальном времени").strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(primary_live_input_label(&self.runtime.raw_state))
                            .size(11.0)
                            .color(MUTED),
                    );
                });
            });
            ui.add_space(6.0);

            for (index, value) in self.runtime.raw_state.axes.iter().enumerate() {
                let normalized = normalize_axis(*value);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("A{}", index + 1)).size(11.0));
                    let bar_width = (ui.available_width() - 58.0).max(100.0);
                    ui.add(
                        egui::ProgressBar::new(((normalized + 1.0) * 0.5).clamp(0.0, 1.0))
                            .desired_width(bar_width),
                    );
                    ui.label(RichText::new(format!("{normalized:.2}")).size(10.0));
                });
            }

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                for (index, pressed) in self.runtime.raw_state.buttons.iter().enumerate() {
                    status_chip(
                        ui,
                        &format!("B{}", index + 1),
                        if *pressed { SUCCESS } else { SURFACE_MUTED },
                        *pressed,
                    );
                }
            });
            ui.add_space(5.0);
            ui.horizontal_wrapped(|ui| {
                for (index, hat) in self.runtime.raw_state.hats.iter().enumerate() {
                    status_chip(
                        ui,
                        &format!("H{} {}", index + 1, hat.label()),
                        if *hat == crate::model::HatDirection::Centered {
                            SURFACE_MUTED
                        } else {
                            WARNING
                        },
                        *hat != crate::model::HatDirection::Centered,
                    );
                }
                for (axis, negative) in &self.runtime.raw_state.paddles {
                    status_chip(
                        ui,
                        &format!(
                            "Лепесток оси {} {}",
                            axis + 1,
                            if *negative { "-" } else { "+" }
                        ),
                        SUCCESS,
                        true,
                    );
                }
            });
        });
    }

    fn render_settings(&mut self, ui: &mut egui::Ui) {
        surface(ui, |ui| {
            ui.label(RichText::new("Настройки и обновления").strong());
            ui.add_space(6.0);

            let mut changed = false;
            ui.columns(2, |columns| {
                changed |= columns[0]
                    .checkbox(&mut self.state.start_in_tray, "Запуск в трее")
                    .changed();
                changed |= columns[0]
                    .checkbox(
                        &mut self.state.forward_only_in_tray,
                        "Работать только в трее",
                    )
                    .changed();
                self.state.block_game_bar_controller_button = true;
                columns[1].add_enabled(
                    false,
                    egui::Checkbox::new(
                        &mut self.state.block_game_bar_controller_button,
                        "Xbox Game Bar заблокирован",
                    ),
                );
                changed |= columns[1]
                    .checkbox(
                        &mut self.state.include_prereleases,
                        "Предварительные версии",
                    )
                    .changed();
            });

            if changed {
                self.sync_forwarding_mode();
                if let Err(error) = game_bar::install_hard_block() {
                    self.status = format!("Не удалось обновить блокировку Xbox Game Bar: {error}");
                }
                self.save();
                self.sync_tray();
            }

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        !config::is_dev_build(),
                        egui::Button::new(if self.state.startup_enabled {
                            "Выключить автозапуск"
                        } else {
                            "Включить автозапуск"
                        }),
                    )
                    .clicked()
                {
                    self.toggle_startup();
                }
                if ui
                    .add_enabled(
                        !config::is_dev_build() && self.update_rx.is_none(),
                        egui::Button::new("Проверить обновления"),
                    )
                    .clicked()
                {
                    self.save();
                    self.start_update_check();
                }
                if ui
                    .add_enabled(
                        self.last_update_check
                            .as_ref()
                            .map(|check| {
                                check.is_update_available && check.asset_download_url.is_some()
                            })
                            .unwrap_or(false),
                        egui::Button::new("Установить обновление"),
                    )
                    .clicked()
                {
                    self.install_update();
                }
                if ui.button("Страница релизов").clicked() {
                    if let Err(error) = updater::open_releases_page() {
                        self.status = format!("Не удалось открыть страницу релизов: {error}");
                    }
                }
            });

            ui.add_space(5.0);
            if config::is_dev_build() {
                ui.label(
                    RichText::new("Проверка обновлений отключена в сборке для разработки.")
                        .size(11.0)
                        .color(MUTED),
                );
            } else if let Some(check) = &self.last_update_check {
                ui.label(RichText::new(update_status(check)).size(11.0).color(MUTED));
            } else {
                ui.label(
                    RichText::new("Обновления ещё не проверялись.")
                        .size(11.0)
                        .color(MUTED),
                );
            }

            ui.add_space(10.0);
            ui.label(RichText::new("Игровые устройства (joy.cpl)").strong());
            ui.label(
                RichText::new(
                    "Некоторые рули и джойстики отдают часть кнопок, только пока открыты их \
                     «Свойства». Они открываются при запуске программы за краем экрана и \
                     закрываются при выходе.",
                )
                .size(11.0)
                .color(MUTED),
            );
            let mut joy_changed = false;
            ui.horizontal(|ui| {
                ui.label(RichText::new("Имя в joy.cpl").size(11.0).color(MUTED));
                joy_changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut self.state.joycpl_device_name)
                            .hint_text("пусто — выбранный контроллер")
                            .desired_width(240.0),
                    )
                    .changed();
            });
            if joy_changed {
                self.save();
            }
            ui.horizontal_wrapped(|ui| {
                let open_label = if self.joycpl_rx.is_some() {
                    "Открываю…"
                } else if self.joycpl_pid.is_some() {
                    "Открыть заново"
                } else {
                    "Открыть свойства (за экраном)"
                };
                if ui
                    .add_enabled(self.joycpl_rx.is_none(), egui::Button::new(open_label))
                    .clicked()
                {
                    self.start_joycpl();
                }
                if ui
                    .add_enabled(self.joycpl_pid.is_some(), egui::Button::new("Закрыть"))
                    .clicked()
                {
                    self.close_joycpl();
                }
            });
        });
    }
}
fn surface(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(12)
        .inner_margin(egui::Margin::same(14))
        .show(ui, add_contents);
}

fn draw_controller_body(ui: &egui::Ui, rect: egui::Rect) {
    let painter = ui.painter();
    let shell = Color32::from_rgb(38, 42, 50);
    let shell_dark = Color32::from_rgb(25, 28, 34);
    let shell_mid = Color32::from_rgb(67, 73, 84);
    let silver = Color32::from_rgb(223, 227, 232);
    let silver_dark = Color32::from_rgb(180, 187, 196);

    let body = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.center().y + 5.0),
        egui::vec2(rect.width() * 0.74, rect.height() * 0.70),
    );

    painter.circle_filled(
        egui::pos2(body.left() + body.width() * 0.12, body.bottom() - 4.0),
        body.height() * 0.34,
        shell,
    );
    painter.circle_filled(
        egui::pos2(body.right() - body.width() * 0.12, body.bottom() - 4.0),
        body.height() * 0.34,
        shell,
    );
    painter.rect_filled(body, 76, shell);
    painter.rect_stroke(
        body,
        76,
        Stroke::new(2.0, shell_mid),
        egui::StrokeKind::Inside,
    );

    painter.rect_filled(
        egui::Rect::from_center_size(
            egui::pos2(body.center().x, body.top() + 30.0),
            egui::vec2(body.width() * 0.35, 16.0),
        ),
        8,
        shell_mid,
    );

    for x in [0.24, 0.76] {
        painter.rect_filled(
            egui::Rect::from_center_size(
                egui::pos2(body.left() + body.width() * x, body.top() + 5.0),
                egui::vec2(72.0, 15.0),
            ),
            7,
            shell_dark,
        );
    }

    let dpad = egui::pos2(body.left() + body.width() * 0.22, body.center().y + 4.0);
    painter.rect_filled(
        egui::Rect::from_center_size(dpad, egui::vec2(72.0, 22.0)),
        8,
        silver,
    );
    painter.rect_filled(
        egui::Rect::from_center_size(dpad, egui::vec2(22.0, 72.0)),
        8,
        silver,
    );
    painter.circle_stroke(dpad, 39.0, Stroke::new(1.5, silver_dark));

    for (center, color) in [
        (
            egui::pos2(body.right() - body.width() * 0.18, body.center().y - 25.0),
            Color32::from_rgb(234, 194, 42),
        ),
        (
            egui::pos2(body.right() - body.width() * 0.11, body.center().y + 14.0),
            Color32::from_rgb(222, 76, 72),
        ),
        (
            egui::pos2(body.right() - body.width() * 0.18, body.center().y + 53.0),
            Color32::from_rgb(83, 161, 55),
        ),
        (
            egui::pos2(body.right() - body.width() * 0.25, body.center().y + 14.0),
            Color32::from_rgb(58, 115, 215),
        ),
    ] {
        painter.circle_filled(center, 18.0, color);
        painter.circle_stroke(center, 18.0, Stroke::new(1.5, Color32::WHITE));
    }

    painter.circle_filled(
        egui::pos2(body.center().x, body.center().y - 17.0),
        18.0,
        silver,
    );
    painter.circle_stroke(
        egui::pos2(body.center().x, body.center().y - 17.0),
        18.0,
        Stroke::new(2.0, Color32::from_rgb(111, 187, 58)),
    );

    for center in [
        egui::pos2(body.center().x - 38.0, body.center().y + 3.0),
        egui::pos2(body.center().x + 38.0, body.center().y + 3.0),
    ] {
        painter.circle_filled(center, 12.0, silver);
        painter.circle_stroke(center, 12.0, Stroke::new(1.0, silver_dark));
    }
}

/// Ширина / высота картинки `controller-layout.png`.
const CONTROLLER_IMAGE_ASPECT: f32 = 506.0 / 333.0;

/// Точки на картинке DualShock 4 (доли ширины/высоты картинки; триггеры — над ней).
/// Подписи остаются Xbox: A — крест, B — круг, X — квадрат, Y — треугольник.
fn controller_points() -> [(OutputControl, f32, f32); 15] {
    [
        (OutputControl::LeftTrigger, 0.217, -0.11),
        (OutputControl::RightTrigger, 0.783, -0.11),
        (OutputControl::LeftShoulder, 0.217, 0.03),
        (OutputControl::RightShoulder, 0.783, 0.03),
        (OutputControl::Guide, 0.498, 0.495),
        (OutputControl::Back, 0.306, 0.12),
        (OutputControl::Start, 0.709, 0.12),
        (OutputControl::Y, 0.814, 0.165),
        (OutputControl::B, 0.887, 0.279),
        (OutputControl::A, 0.814, 0.384),
        (OutputControl::X, 0.743, 0.276),
        (OutputControl::DpadUp, 0.194, 0.204),
        (OutputControl::DpadRight, 0.253, 0.279),
        (OutputControl::DpadDown, 0.194, 0.360),
        (OutputControl::DpadLeft, 0.134, 0.279),
    ]
}

fn short_control_label(control: OutputControl) -> &'static str {
    match control {
        OutputControl::A => "A",
        OutputControl::B => "B",
        OutputControl::X => "X",
        OutputControl::Y => "Y",
        OutputControl::LeftShoulder => "LB",
        OutputControl::RightShoulder => "RB",
        OutputControl::Back => "BACK",
        OutputControl::Start => "START",
        OutputControl::Guide => "GUIDE",
        OutputControl::LeftThumb => "L3",
        OutputControl::RightThumb => "R3",
        OutputControl::DpadUp => "",
        OutputControl::DpadRight => "",
        OutputControl::DpadDown => "",
        OutputControl::DpadLeft => "",
        OutputControl::LeftTrigger => "LT",
        OutputControl::RightTrigger => "RT",
        OutputControl::LeftStickX => "LX",
        OutputControl::LeftStickY => "LY",
        OutputControl::RightStickX => "RX",
        OutputControl::RightStickY => "RY",
    }
}

fn ellipsize(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_owned();
    }
    let keep = max_chars.saturating_sub(1);
    let mut shortened = value.chars().take(keep).collect::<String>();
    shortened.push('…');
    shortened
}

fn normalize_axis(value: i16) -> f32 {
    if value < 0 {
        value as f32 / 32_768.0
    } else {
        value as f32 / 32_767.0
    }
}

fn ui_root(app: &mut X360ceApp, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
    ui.painter().rect_filled(ui.max_rect(), 0.0, BACKGROUND);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                ui.vertical(|ui| {
                    ui.set_max_width((ui.available_width() - 12.0).max(1040.0));
                    app.render_header(ui);
                    ui.add_space(8.0);
                    app.render_status(ui);
                    ui.add_space(8.0);

                    ui.columns(2, |columns| {
                        let (left, right) = columns.split_at_mut(1);
                        left[0].vertical(|ui| {
                            app.render_controller(ui);
                            ui.add_space(8.0);
                            app.render_layers(ui);
                            ui.add_space(8.0);
                            app.render_keyboard(ui);
                            ui.add_space(8.0);
                            app.render_paddles(ui);
                            ui.add_space(8.0);
                            app.render_raw_inputs(ui);
                        });
                        right[0].vertical(|ui| {
                            app.render_profiles(ui);
                            ui.add_space(8.0);
                            app.render_device_bar(ui);
                            ui.add_space(8.0);
                            app.render_mapping_editor(ui);
                            ui.add_space(8.0);
                            app.render_settings(ui);
                        });
                    });
                    ui.add_space(12.0);
                });
            });
        });
}

fn compact_surface(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(12)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, add_contents);
}

#[derive(Clone, Copy)]
enum DirectionIcon {
    Up,
    Right,
    Down,
    Left,
}

fn control_direction_icon(control: OutputControl) -> Option<DirectionIcon> {
    match control {
        OutputControl::DpadUp => Some(DirectionIcon::Up),
        OutputControl::DpadRight => Some(DirectionIcon::Right),
        OutputControl::DpadDown => Some(DirectionIcon::Down),
        OutputControl::DpadLeft => Some(DirectionIcon::Left),
        _ => None,
    }
}

fn draw_direction_icon(
    painter: &egui::Painter,
    center: egui::Pos2,
    direction: DirectionIcon,
    color: Color32,
    scale: f32,
    stroke_width: f32,
) {
    let forward = match direction {
        DirectionIcon::Up => egui::vec2(0.0, -1.0),
        DirectionIcon::Right => egui::vec2(1.0, 0.0),
        DirectionIcon::Down => egui::vec2(0.0, 1.0),
        DirectionIcon::Left => egui::vec2(-1.0, 0.0),
    };
    let side = egui::vec2(-forward.y, forward.x);
    let tip = center + forward * (scale * 0.55);
    let tail = center - forward * (scale * 0.45);
    let head = tip - forward * (scale * 0.45);
    painter.line_segment([tail, tip], Stroke::new(stroke_width, color));
    painter.line_segment(
        [tip, head + side * (scale * 0.34)],
        Stroke::new(stroke_width, color),
    );
    painter.line_segment(
        [tip, head - side * (scale * 0.34)],
        Stroke::new(stroke_width, color),
    );
}

fn controller_control(
    app: &mut X360ceApp,
    ui: &mut egui::Ui,
    profile: &ControllerProfile,
    control: OutputControl,
    center: egui::Pos2,
    radius: f32,
    label: &str,
) {
    let hit = egui::Rect::from_center_size(center, egui::vec2(radius * 2.8, radius * 2.8));
    let response = ui.interact(
        hit,
        ui.make_persistent_id(("controller-control", control)),
        Sense::click(),
    );
    if response.double_clicked() {
        app.begin_learning(control);
    } else if response.clicked() {
        app.selected_control = control;
        app.selected_axis_negative = None;
    }

    let entry = profile.entry(control);
    let mapped = entry
        .map(|entry| entry.binding != InputBinding::None)
        .unwrap_or(false);
    let active = entry
        .map(|entry| control_preview_active(control, entry, &app.runtime.raw_state))
        .unwrap_or(false);
    let selected = app.learning_control == Some(control) && app.selected_axis_negative.is_none();
    // В слое модификатора пустые назначения — норма, не подсвечиваем их.
    let mapped = mapped || app.edit_layer.is_some();
    let fill = if active {
        Color32::from_rgb(55, 184, 105)
    } else if selected {
        Color32::from_rgba_unmultiplied(52, 111, 224, 225)
    } else if mapped {
        Color32::from_rgba_unmultiplied(255, 255, 255, 245)
    } else {
        Color32::from_rgba_unmultiplied(255, 232, 187, 230)
    };
    let stroke = if active {
        Stroke::new(1.8, Color32::from_rgb(29, 110, 61))
    } else if selected {
        Stroke::new(2.4, ACCENT)
    } else if mapped {
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(24, 29, 37, 92))
    } else {
        Stroke::new(1.2, Color32::from_rgba_unmultiplied(205, 125, 21, 180))
    };
    ui.painter().circle_filled(center, radius, fill);
    ui.painter().circle_stroke(center, radius, stroke);
    let label_color = if active {
        Color32::from_rgb(18, 24, 21)
    } else if selected {
        Color32::WHITE
    } else if mapped || control == OutputControl::Y {
        Color32::from_rgb(24, 29, 37)
    } else {
        Color32::from_rgb(143, 85, 14)
    };
    if let Some(direction) = control_direction_icon(control) {
        draw_direction_icon(
            ui.painter(),
            center,
            direction,
            label_color,
            radius * 0.95,
            if active { 2.4 } else { 1.9 },
        );
    } else {
        ui.painter().text(
            center,
            Align2::CENTER_CENTER,
            label,
            FontId::proportional(
                if matches!(
                    control,
                    OutputControl::Start | OutputControl::Back | OutputControl::Guide
                ) {
                    7.2
                } else if active {
                    10.0
                } else {
                    9.0
                },
            ),
            label_color,
        );
    }
    if !mapped {
        ui.painter().text(
            center + egui::vec2(radius * 0.8, -radius * 0.8),
            Align2::CENTER_CENTER,
            "!",
            FontId::proportional(8.0),
            WARNING,
        );
    }
    response.on_hover_text(format!(
        "{}
{}
Двойной щелчок — назначить",
        control.label(),
        entry_label_for(profile, control)
    ));
}

fn draw_stick_controls(
    app: &mut X360ceApp,
    ui: &mut egui::Ui,
    profile: &ControllerProfile,
    left: bool,
    center: egui::Pos2,
) {
    let (x_control, y_control, click_control, name) = if left {
        (
            OutputControl::LeftStickX,
            OutputControl::LeftStickY,
            OutputControl::LeftThumb,
            "L",
        )
    } else {
        (
            OutputControl::RightStickX,
            OutputControl::RightStickY,
            OutputControl::RightThumb,
            "R",
        )
    };

    let x_value = profile
        .entry(x_control)
        .map(|entry| mapper::axis_preview(entry, &app.runtime.raw_state))
        .unwrap_or_default();
    let y_value = profile
        .entry(y_control)
        .map(|entry| mapper::axis_preview(entry, &app.runtime.raw_state))
        .unwrap_or_default();

    controller_control(
        app,
        ui,
        profile,
        click_control,
        center,
        10.0,
        &format!("{name}3"),
    );

    for (control, output_negative, offset, direction_id, direction, active) in [
        (
            y_control,
            false,
            egui::vec2(0.0, -28.0),
            "up",
            DirectionIcon::Up,
            y_value > 0.15,
        ),
        (
            x_control,
            false,
            egui::vec2(28.0, 0.0),
            "right",
            DirectionIcon::Right,
            x_value > 0.15,
        ),
        (
            y_control,
            true,
            egui::vec2(0.0, 28.0),
            "down",
            DirectionIcon::Down,
            y_value < -0.15,
        ),
        (
            x_control,
            true,
            egui::vec2(-28.0, 0.0),
            "left",
            DirectionIcon::Left,
            x_value < -0.15,
        ),
    ] {
        let marker_center = center + offset + egui::vec2(0.0, -4.0);
        let hit = egui::Rect::from_center_size(marker_center, egui::vec2(20.0, 20.0));
        let response = ui.interact(
            hit,
            ui.make_persistent_id(("stick-direction", left, direction_id)),
            Sense::click(),
        );
        if response.double_clicked() {
            app.begin_learning_with_direction(control, output_negative);
        } else if response.clicked() {
            app.selected_control = control;
            app.selected_axis_negative = Some(output_negative);
        }

        let entry = profile.entry(control);
        let mapped = entry
            .map(|entry| entry.binding != InputBinding::None)
            .unwrap_or(false);
        let selected = app.learning_control == Some(control)
            && app.selected_axis_negative == Some(output_negative);
        let fill = if active {
            Color32::from_rgb(55, 184, 105)
        } else if selected {
            Color32::from_rgba_unmultiplied(52, 111, 224, 225)
        } else if mapped {
            Color32::from_rgba_unmultiplied(255, 255, 255, 245)
        } else {
            Color32::from_rgba_unmultiplied(255, 232, 187, 230)
        };
        ui.painter().circle_filled(marker_center, 8.5, fill);
        ui.painter().circle_stroke(
            marker_center,
            8.5,
            Stroke::new(
                if selected { 2.0 } else { 1.0 },
                if active {
                    Color32::from_rgb(29, 110, 61)
                } else if selected {
                    ACCENT
                } else if mapped {
                    Color32::from_rgba_unmultiplied(24, 29, 37, 92)
                } else {
                    Color32::from_rgba_unmultiplied(205, 125, 21, 175)
                },
            ),
        );
        let icon_color = if active {
            Color32::from_rgb(18, 24, 21)
        } else if selected {
            Color32::WHITE
        } else if mapped {
            Color32::from_rgb(24, 29, 37)
        } else {
            Color32::from_rgb(143, 85, 14)
        };
        draw_direction_icon(
            ui.painter(),
            marker_center,
            direction,
            icon_color,
            7.0,
            if active { 2.1 } else { 1.7 },
        );
        response.on_hover_text(format!(
            "{} — {} направление
Двойной щелчок — назначить",
            control.label(),
            if output_negative {
                "отрицательное"
            } else {
                "положительное"
            }
        ));
    }
}

fn compact_control_label(control: OutputControl) -> &'static str {
    match control {
        OutputControl::LeftShoulder => "LB",
        OutputControl::RightShoulder => "RB",
        OutputControl::LeftThumb => "L3",
        OutputControl::RightThumb => "R3",
        OutputControl::DpadUp => "Крестовина вверх",
        OutputControl::DpadRight => "Крестовина вправо",
        OutputControl::DpadDown => "Крестовина вниз",
        OutputControl::DpadLeft => "Крестовина влево",
        OutputControl::LeftTrigger => "LT",
        OutputControl::RightTrigger => "RT",
        OutputControl::LeftStickX => "Левый X",
        OutputControl::LeftStickY => "Левый Y",
        OutputControl::RightStickX => "Правый X",
        OutputControl::RightStickY => "Правый Y",
        _ => control.label(),
    }
}

fn status_chip(ui: &mut egui::Ui, text: &str, fill: Color32, strong: bool) {
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::NONE)
        .corner_radius(14)
        .inner_margin(egui::Margin::symmetric(9, 4))
        .show(ui, |ui| {
            let text_color = if strong && fill != SURFACE_MUTED && fill != ACCENT_SOFT {
                Color32::WHITE
            } else {
                TEXT
            };
            ui.label(RichText::new(text).size(11.0).strong().color(text_color));
        });
}

fn control_preview_active(
    control: OutputControl,
    entry: &crate::model::MappingEntry,
    raw: &RawState,
) -> bool {
    if control.is_trigger() {
        mapper::trigger_preview(entry, raw) > 0.15
    } else if control.is_axis() {
        mapper::axis_preview(entry, raw).abs() > 0.15
    } else {
        mapper::binding_pressed(entry, raw)
    }
}

fn primary_live_input_label(raw: &RawState) -> String {
    if let Some((axis, negative)) = raw.paddles.first() {
        return InputBinding::Paddle {
            axis: *axis,
            negative: *negative,
        }
        .label();
    }
    for (index, pressed) in raw.buttons.iter().enumerate() {
        if *pressed {
            return format!("Кнопка {}", index + 1);
        }
    }
    for (index, value) in raw.axes.iter().enumerate() {
        let normalized = normalize_axis(*value);
        if normalized >= 0.18 {
            return format!("Ось {} +", index + 1);
        }
        if normalized <= -0.18 {
            return format!("Ось {} -", index + 1);
        }
    }
    for (index, hat) in raw.hats.iter().enumerate() {
        if *hat != crate::model::HatDirection::Centered {
            return format!("Хатка {} {}", index + 1, hat.label());
        }
    }
    "ничего".to_owned()
}

fn axis_binding_positive(binding: InputBinding) -> Option<bool> {
    match binding {
        InputBinding::AxisPositive { .. } => Some(true),
        InputBinding::AxisNegative { .. } => Some(false),
        _ => None,
    }
}

fn load_controller_texture(ctx: &egui::Context) -> Option<TextureHandle> {
    let image = image::load_from_memory(include_bytes!("../assets/controller-layout.png"))
        .ok()?
        .into_rgba8();
    let (width, height) = image.dimensions();
    let color_image =
        egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], image.as_raw());
    Some(ctx.load_texture(
        "x360ce-controller-layout",
        color_image,
        TextureOptions::LINEAR,
    ))
}

/// Строка «покой оси / сейчас» для оси назначения. true — нажали «Запомнить».
fn rest_row(ui: &mut egui::Ui, raw: &RawState, binding: InputBinding) -> bool {
    let (InputBinding::AxisPositive { index } | InputBinding::AxisNegative { index }) = binding
    else {
        return false;
    };
    let percent = |value: i16| (value as f32 / 327.67).round() as i32;
    let rest = raw.rest.get(index as usize).copied().flatten();
    let now = raw.axes.get(index as usize).copied();
    let travel = mapper::axis_travel(
        raw,
        index,
        matches!(binding, InputBinding::AxisNegative { .. }),
        0,
    );
    let mut clicked = false;
    ui.horizontal_wrapped(|ui| {
        let text = match (rest, now) {
            (Some(rest), Some(now)) => format!(
                "Ось {}: покой {}%, сейчас {}%, нажато {}%",
                index + 1,
                percent(rest),
                percent(now),
                (travel * 100.0).round()
            ),
            (None, _) => format!("Ось {}: покой не запомнен", index + 1),
            (_, None) => format!("Ось {}: нет у контроллера", index + 1),
        };
        ui.label(RichText::new(text).size(11.0).color(if rest.is_some() { MUTED } else { WARNING }));
        clicked = ui
            .small_button("Запомнить покой")
            .on_hover_text("Отпустите педали и нажмите")
            .clicked();
    });
    clicked
}

/// Назначения слоя `layer`, срабатывающие от того же физического входа.
fn conflicts_for(
    profile: &ControllerProfile,
    layer: Option<usize>,
    binding: InputBinding,
    full: bool,
    skip_control: Option<OutputControl>,
    skip_key: Option<usize>,
) -> (Vec<OutputControl>, Vec<usize>) {
    let controls = profile
        .layer_entries(layer)
        .iter()
        .filter(|entry| Some(entry.control) != skip_control)
        .filter(|entry| inputs_overlap(binding, full, entry.binding, entry.uses_full_axis()))
        .map(|entry| entry.control)
        .collect();
    let keys = profile
        .keyboard
        .iter()
        .enumerate()
        .filter(|(index, mapping)| {
            Some(*index) != skip_key
                && mapping.modifier == layer
                && mapping.key.vk != 0
                && inputs_overlap(binding, full, mapping.input, false)
        })
        .map(|(index, _)| index)
        .collect();
    (controls, keys)
}

fn conflict_names(profile: &ControllerProfile, controls: &[OutputControl], keys: &[usize]) -> String {
    controls
        .iter()
        .map(|control| compact_control_label(*control).to_owned())
        .chain(keys.iter().filter_map(|index| {
            profile
                .keyboard
                .get(*index)
                .map(|mapping| format!("клавиша {}", mapping.key.label()))
        }))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Назначения совпадают (имя и устройство не сравниваются).
fn same_mappings(a: &ControllerProfile, b: &ControllerProfile) -> bool {
    let strip = |profile: &ControllerProfile| {
        let mut profile = profile.clone();
        profile.device_guid.clear();
        serde_json::to_value(profile).ok()
    };
    strip(a) == strip(b)
}

fn entry_label_for(profile: &ControllerProfile, control: OutputControl) -> String {
    profile
        .entry(control)
        .map(|entry| {
            let label = entry.binding.label();
            if entry.trigger == TriggerType::Direct || entry.binding == InputBinding::None {
                label
            } else {
                format!("{label} · {}", entry.trigger.label())
            }
        })
        .unwrap_or_else(|| "Не назначено".to_owned())
}

fn draw_embedded_logo(ui: &mut egui::Ui) {
    let image = image::load_from_memory(include_bytes!("../assets/x360ce.png"))
        .ok()
        .map(|image| image.into_rgba8());

    if let Some(image) = image {
        let (width, height) = image.dimensions();
        let color_image = egui::ColorImage::from_rgba_unmultiplied(
            [width as usize, height as usize],
            image.as_raw(),
        );
        let texture = ui.ctx().load_texture(
            "x360ce-embedded-logo",
            color_image,
            egui::TextureOptions::LINEAR,
        );
        ui.add(egui::widgets::Image::new(&texture).fit_to_exact_size(egui::vec2(56.0, 56.0)));
    } else {
        draw_header_logo(ui);
    }
}

fn configure_egui(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Light);
    let mut visuals = egui::Visuals::light();
    visuals.panel_fill = BACKGROUND;
    visuals.window_fill = SURFACE;
    visuals.faint_bg_color = SURFACE_MUTED;
    visuals.selection.bg_fill = ACCENT_SOFT;
    visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    visuals.widgets.active.bg_fill = ACCENT;
    visuals.widgets.hovered.bg_fill = ACCENT_SOFT;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    ctx.set_visuals(visuals);
}

fn draw_header_logo(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(52.0, 52.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 12, CONTROLLER);
    let center = rect.center();
    painter.line_segment(
        [
            egui::pos2(center.x - 13.0, center.y),
            egui::pos2(center.x - 3.0, center.y),
        ],
        Stroke::new(4.0, Color32::WHITE),
    );
    painter.line_segment(
        [
            egui::pos2(center.x - 8.0, center.y - 5.0),
            egui::pos2(center.x - 8.0, center.y + 5.0),
        ],
        Stroke::new(4.0, Color32::WHITE),
    );
    painter.circle_filled(egui::pos2(center.x + 10.0, center.y - 5.0), 3.5, WARNING);
    painter.circle_filled(egui::pos2(center.x + 16.0, center.y + 1.0), 3.5, DANGER);
    painter.circle_filled(egui::pos2(center.x + 10.0, center.y + 7.0), 3.5, SUCCESS);
    painter.circle_filled(egui::pos2(center.x + 4.0, center.y + 1.0), 3.5, ACCENT);
}

fn load_window_icon(bytes: &[u8]) -> Result<egui::IconData> {
    let image = image::load_from_memory(bytes)
        .context("не удалось загрузить значок окна")?
        .into_rgba8();
    let (width, height) = image.dimensions();
    Ok(egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}

fn load_tray_image(bytes: &[u8]) -> Result<TrayImage> {
    let image = image::load_from_memory(bytes)
        .context("не удалось загрузить значок трея")?
        .into_rgba8();
    let (width, height) = image.dimensions();
    TrayImage::from_rgba(image.into_raw(), width, height)
        .map_err(|error| anyhow::anyhow!("неверный значок трея: {error}"))
}

fn tray_tooltip(state: &SavedState, virtual_connected: bool) -> String {
    format!(
        "x360ce {} — {}",
        config::app_version_label(),
        if virtual_connected {
            "работает"
        } else if state.emulation_enabled {
            "ожидание контроллера"
        } else {
            "пауза"
        }
    )
}

fn update_status(check: &updater::UpdateCheck) -> String {
    let channel = if check.prerelease {
        "предварительная"
    } else {
        "стабильная"
    };
    if check.is_update_available {
        match &check.asset_name {
            Some(asset) => format!(
                "Доступно обновление: v{} ({channel}, {asset}).",
                check.latest_version
            ),
            None => format!(
                "Доступно обновление: v{} ({channel}), но файл для Windows не найден.",
                check.latest_version
            ),
        }
    } else {
        format!(
            "Установлена последняя версия: v{} ({channel}).",
            check.current_version
        )
    }
}

fn press_mode_label(mode: PressMode) -> &'static str {
    match mode {
        PressMode::HoldWhilePressed => "Удерживать, пока зажата",
        PressMode::Pulse => "Короткий импульс",
    }
}

/// Клавиша egui → виртуальный код Windows.
fn egui_key_to_vk(key: egui::Key) -> Option<u16> {
    use egui::Key as K;
    let letters = [
        K::A,
        K::B,
        K::C,
        K::D,
        K::E,
        K::F,
        K::G,
        K::H,
        K::I,
        K::J,
        K::K,
        K::L,
        K::M,
        K::N,
        K::O,
        K::P,
        K::Q,
        K::R,
        K::S,
        K::T,
        K::U,
        K::V,
        K::W,
        K::X,
        K::Y,
        K::Z,
    ];
    if let Some(index) = letters.iter().position(|item| *item == key) {
        return Some(0x41 + index as u16);
    }
    let digits = [
        K::Num0,
        K::Num1,
        K::Num2,
        K::Num3,
        K::Num4,
        K::Num5,
        K::Num6,
        K::Num7,
        K::Num8,
        K::Num9,
    ];
    if let Some(index) = digits.iter().position(|item| *item == key) {
        return Some(0x30 + index as u16);
    }
    let functions = [
        K::F1,
        K::F2,
        K::F3,
        K::F4,
        K::F5,
        K::F6,
        K::F7,
        K::F8,
        K::F9,
        K::F10,
        K::F11,
        K::F12,
    ];
    if let Some(index) = functions.iter().position(|item| *item == key) {
        return Some(0x70 + index as u16);
    }
    Some(match key {
        K::Space => 0x20,
        K::Enter => 0x0D,
        K::Escape => 0x1B,
        K::Tab => 0x09,
        K::Backspace => 0x08,
        K::ArrowUp => 0x26,
        K::ArrowDown => 0x28,
        K::ArrowLeft => 0x25,
        K::ArrowRight => 0x27,
        K::Insert => 0x2D,
        K::Delete => 0x2E,
        K::Home => 0x24,
        K::End => 0x23,
        K::PageUp => 0x21,
        K::PageDown => 0x22,
        K::ShiftLeft => 0xA0,
        K::ShiftRight => 0xA1,
        K::ControlLeft => 0xA2,
        K::ControlRight => 0xA3,
        K::AltLeft => 0xA4,
        K::AltRight => 0xA5,
        K::Minus => 0xBD,
        K::Equals | K::Plus => 0xBB,
        K::OpenBracket => 0xDB,
        K::CloseBracket => 0xDD,
        K::Semicolon => 0xBA,
        K::Quote => 0xDE,
        K::Comma => 0xBC,
        K::Period => 0xBE,
        K::Slash => 0xBF,
        K::Backslash => 0xDC,
        K::Backtick => 0xC0,
        _ => return None,
    })
}

/// Открыть joy.cpl в фоне: до `attempts` попыток с паузой. `device_name`
/// спрашивается на каждой попытке — контроллер может появиться позже.
fn spawn_joycpl(
    previous: Option<u32>,
    attempts: u32,
    device_name: impl Fn() -> Option<String> + Send + 'static,
) -> Receiver<Result<u32, String>> {
    let (sender, receiver) = unbounded();
    thread::spawn(move || {
        if let Some(pid) = previous {
            joycpl::close(pid);
        }
        let mut result = Err("контроллер не найден".to_owned());
        for attempt in 0..attempts {
            if attempt > 0 {
                thread::sleep(JOYCPL_RETRY_PAUSE);
            }
            let Some(name) = device_name() else {
                continue;
            };
            result = joycpl::open(&name);
            if result.is_ok() {
                break;
            }
        }
        let _ = sender.send(result);
    });
    receiver
}
