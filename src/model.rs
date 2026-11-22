use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceDescriptor {
    pub instance_id: u32,
    pub name: String,
    pub guid: String,
    pub axes: u32,
    pub buttons: u32,
    pub hats: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HatDirection {
    #[default]
    Centered,
    Up,
    Right,
    Down,
    Left,
    RightUp,
    RightDown,
    LeftUp,
    LeftDown,
}

impl HatDirection {
    pub fn label(self) -> &'static str {
        match self {
            Self::Centered => "Центр",
            Self::Up => "Вверх",
            Self::Right => "Вправо",
            Self::Down => "Вниз",
            Self::Left => "Влево",
            Self::RightUp => "Вверх + вправо",
            Self::RightDown => "Вниз + вправо",
            Self::LeftUp => "Вверх + влево",
            Self::LeftDown => "Вниз + влево",
        }
    }

    pub fn contains(self, expected: Self) -> bool {
        match expected {
            Self::Up => matches!(self, Self::Up | Self::RightUp | Self::LeftUp),
            Self::Right => matches!(self, Self::Right | Self::RightUp | Self::RightDown),
            Self::Down => matches!(self, Self::Down | Self::RightDown | Self::LeftDown),
            Self::Left => matches!(self, Self::Left | Self::LeftUp | Self::LeftDown),
            _ => self == expected,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawState {
    #[serde(default)]
    pub axes: Vec<i16>,
    #[serde(default)]
    pub buttons: Vec<bool>,
    #[serde(default)]
    pub hats: Vec<HatDirection>,
    /// Зажатые лепестки на общей с педалью оси: (индекс оси, отрицательное направление).
    #[serde(default)]
    pub paddles: Vec<(u32, bool)>,
    /// Положение покоя каждой оси (из профиля); None — неизвестно.
    #[serde(default)]
    pub rest: Vec<Option<i16>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum InputBinding {
    #[default]
    None,
    Button {
        index: u32,
    },
    AxisPositive {
        index: u32,
    },
    AxisNegative {
        index: u32,
    },
    Hat {
        index: u32,
        direction: HatDirection,
    },
    /// Лепесток, отделённый от педали на той же оси (см. [`PaddleAxis`]).
    Paddle {
        axis: u32,
        negative: bool,
    },
}

impl InputBinding {
    pub fn label(&self) -> String {
        match self {
            Self::None => "Не назначено".to_owned(),
            Self::Button { index } => format!("Кнопка {}", index + 1),
            Self::AxisPositive { index } => format!("Ось {} +", index + 1),
            Self::AxisNegative { index } => format!("Ось {} -", index + 1),
            Self::Hat { index, direction } => {
                format!("Хатка {} {}", index + 1, direction.label())
            }
            Self::Paddle { axis, negative } => {
                format!(
                    "Лепесток оси {} {}",
                    axis + 1,
                    if *negative { "-" } else { "+" }
                )
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutputControl {
    #[default]
    A,
    B,
    X,
    Y,
    LeftShoulder,
    RightShoulder,
    Back,
    Start,
    Guide,
    LeftThumb,
    RightThumb,
    DpadUp,
    DpadRight,
    DpadDown,
    DpadLeft,
    LeftTrigger,
    RightTrigger,
    LeftStickX,
    LeftStickY,
    RightStickX,
    RightStickY,
}

impl OutputControl {
    pub const BUTTONS: [Self; 15] = [
        Self::A,
        Self::B,
        Self::X,
        Self::Y,
        Self::LeftShoulder,
        Self::RightShoulder,
        Self::Back,
        Self::Start,
        Self::Guide,
        Self::LeftThumb,
        Self::RightThumb,
        Self::DpadUp,
        Self::DpadRight,
        Self::DpadDown,
        Self::DpadLeft,
    ];

    pub const ANALOGS: [Self; 6] = [
        Self::LeftTrigger,
        Self::RightTrigger,
        Self::LeftStickX,
        Self::LeftStickY,
        Self::RightStickX,
        Self::RightStickY,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::X => "X",
            Self::Y => "Y",
            Self::LeftShoulder => "Левый бампер (LB)",
            Self::RightShoulder => "Правый бампер (RB)",
            Self::Back => "Back",
            Self::Start => "Start",
            Self::Guide => "Guide",
            Self::LeftThumb => "Нажатие левого стика (L3)",
            Self::RightThumb => "Нажатие правого стика (R3)",
            Self::DpadUp => "Крестовина вверх",
            Self::DpadRight => "Крестовина вправо",
            Self::DpadDown => "Крестовина вниз",
            Self::DpadLeft => "Крестовина влево",
            Self::LeftTrigger => "Левый триггер (LT)",
            Self::RightTrigger => "Правый триггер (RT)",
            Self::LeftStickX => "Левый стик X",
            Self::LeftStickY => "Левый стик Y",
            Self::RightStickX => "Правый стик X",
            Self::RightStickY => "Правый стик Y",
        }
    }

    pub fn is_axis(self) -> bool {
        matches!(
            self,
            Self::LeftStickX | Self::LeftStickY | Self::RightStickX | Self::RightStickY
        )
    }

    pub fn is_trigger(self) -> bool {
        matches!(self, Self::LeftTrigger | Self::RightTrigger)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MappingEntry {
    pub control: OutputControl,
    #[serde(default)]
    pub binding: InputBinding,
    #[serde(default)]
    pub invert: bool,
    #[serde(default)]
    pub centered_axis: bool,
    #[serde(default = "default_deadzone")]
    pub deadzone: f32,
    #[serde(default = "default_saturation")]
    pub saturation: f32,
    /// Резкость отклика 0.01..=1.0: 1 — линейно, меньше — плавнее у центра.
    #[serde(default = "default_sharpness")]
    pub sharpness: f32,
    /// Тип нажатия; для стиков всегда используется Direct.
    #[serde(default)]
    pub trigger: TriggerType,
}

/// Как физическое нажатие превращается в действие.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TriggerType {
    /// Мгновенно, пока кнопка зажата (поведение по умолчанию).
    #[default]
    Direct,
    SingleTap,
    DoubleTap,
    LongPress,
}

impl TriggerType {
    pub const ALL: [Self; 4] = [
        Self::Direct,
        Self::SingleTap,
        Self::DoubleTap,
        Self::LongPress,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Direct => "Прямое",
            Self::SingleTap => "Одиночное",
            Self::DoubleTap => "Двойное",
            Self::LongPress => "Удержание",
        }
    }
}

/// Что делает сама кнопка-модификатор.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModifierMode {
    /// Только переключает слой, своих действий нет.
    #[default]
    LayerOnly,
    /// Переключает слой; если её нажали и отпустили без комбинации —
    /// выполняет свои действия основного слоя коротким импульсом.
    TapAction,
    /// Переключает слой и одновременно работает как обычная кнопка.
    Passthrough,
}

impl ModifierMode {
    pub const ALL: [Self; 3] = [Self::LayerOnly, Self::TapAction, Self::Passthrough];

    pub fn label(self) -> &'static str {
        match self {
            Self::LayerOnly => "Только слой",
            Self::TapAction => "Слой + своё действие при одиночном нажатии",
            Self::Passthrough => "Слой + обычная кнопка",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ModifierConfig {
    /// Имя, заданное пользователем; пустое — «Модификатор N».
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub binding: InputBinding,
    #[serde(default)]
    pub mode: ModifierMode,
    /// Назначения Xbox 360, действующие пока модификатор зажат.
    #[serde(default)]
    pub entries: Vec<MappingEntry>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PressMode {
    /// Клавиша зажата, пока зажата кнопка на контроллере.
    #[default]
    HoldWhilePressed,
    /// Короткое нажатие и отпускание.
    Pulse,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyOutput {
    /// Виртуальный код клавиши Windows (VIRTUAL_KEY).
    #[serde(default)]
    pub vk: u16,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub win: bool,
    #[serde(default)]
    pub mode: PressMode,
}

impl KeyOutput {
    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        for (on, name) in [
            (self.ctrl, "Ctrl"),
            (self.shift, "Shift"),
            (self.alt, "Alt"),
            (self.win, "Win"),
        ] {
            if on {
                parts.push(name.to_owned());
            }
        }
        parts.push(key_name(self.vk));
        parts.join(" + ")
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct KeyboardMapping {
    #[serde(default)]
    pub input: InputBinding,
    /// None — основной слой, Some(n) — слой модификатора n.
    #[serde(default)]
    pub modifier: Option<usize>,
    #[serde(default)]
    pub trigger: TriggerType,
    #[serde(default)]
    pub key: KeyOutput,
    /// Название действия, заданное пользователем (например «Шасси»).
    #[serde(default)]
    pub action: String,
}

/// Направление движения мыши (обзор камеры кнопками).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MouseDirection {
    Left,
    Right,
    Up,
    Down,
}

impl MouseDirection {
    pub const ALL: [Self; 4] = [Self::Left, Self::Right, Self::Up, Self::Down];

    pub fn label(self) -> &'static str {
        match self {
            Self::Left => "Мышь влево",
            Self::Right => "Мышь вправо",
            Self::Up => "Мышь вверх",
            Self::Down => "Мышь вниз",
        }
    }
}

/// Скорость мыши 100 % — пикселей в секунду; 1 % — сотая часть.
pub const MOUSE_MAX_PX_PER_SEC: f32 = 2000.0;

/// Пока кнопка зажата, мышь движется в `direction` со скоростью `speed` %.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MouseMapping {
    #[serde(default)]
    pub input: InputBinding,
    /// None — основной слой, Some(n) — слой модификатора n.
    #[serde(default)]
    pub modifier: Option<usize>,
    pub direction: MouseDirection,
    /// 1..=100 %.
    #[serde(default = "default_mouse_speed")]
    pub speed: u8,
}

pub fn default_mouse_speed() -> u8 {
    30
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timings {
    #[serde(default = "default_double_tap_window_ms")]
    pub double_tap_window_ms: u32,
    #[serde(default = "default_hold_threshold_ms")]
    pub hold_threshold_ms: u32,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            double_tap_window_ms: default_double_tap_window_ms(),
            hold_threshold_ms: default_hold_threshold_ms(),
        }
    }
}

fn default_double_tap_window_ms() -> u32 {
    250
}

fn default_hold_threshold_ms() -> u32 {
    350
}

/// Популярные клавиши для выпадающего списка: (VK, название).
#[rustfmt::skip]
pub const KEYS: &[(u16, &str)] = &[
    (0x41, "A"), (0x42, "B"), (0x43, "C"), (0x44, "D"), (0x45, "E"), (0x46, "F"),
    (0x47, "G"), (0x48, "H"), (0x49, "I"), (0x4A, "J"), (0x4B, "K"), (0x4C, "L"),
    (0x4D, "M"), (0x4E, "N"), (0x4F, "O"), (0x50, "P"), (0x51, "Q"), (0x52, "R"),
    (0x53, "S"), (0x54, "T"), (0x55, "U"), (0x56, "V"), (0x57, "W"), (0x58, "X"),
    (0x59, "Y"), (0x5A, "Z"),
    (0x30, "0"), (0x31, "1"), (0x32, "2"), (0x33, "3"), (0x34, "4"),
    (0x35, "5"), (0x36, "6"), (0x37, "7"), (0x38, "8"), (0x39, "9"),
    (0x70, "F1"), (0x71, "F2"), (0x72, "F3"), (0x73, "F4"), (0x74, "F5"), (0x75, "F6"),
    (0x76, "F7"), (0x77, "F8"), (0x78, "F9"), (0x79, "F10"), (0x7A, "F11"), (0x7B, "F12"),
    (0x20, "Пробел"), (0x0D, "Enter"), (0x1B, "Esc"), (0x09, "Tab"), (0x08, "Backspace"),
    (0x26, "Стрелка вверх"), (0x28, "Стрелка вниз"), (0x25, "Стрелка влево"),
    (0x27, "Стрелка вправо"),
    (0x2D, "Insert"), (0x2E, "Delete"), (0x24, "Home"), (0x23, "End"),
    (0x21, "Page Up"), (0x22, "Page Down"),
    (0xA0, "Левый Shift"), (0xA2, "Левый Ctrl"), (0xA4, "Левый Alt"),
    (0xA1, "Правый Shift"), (0xA3, "Правый Ctrl"), (0xA5, "Правый Alt"),
    (0x14, "Caps Lock"),
    (0x60, "Num 0"), (0x61, "Num 1"), (0x62, "Num 2"), (0x63, "Num 3"), (0x64, "Num 4"),
    (0x65, "Num 5"), (0x66, "Num 6"), (0x67, "Num 7"), (0x68, "Num 8"), (0x69, "Num 9"),
    (0x6A, "Num *"), (0x6B, "Num +"), (0x6D, "Num -"), (0x6E, "Num ."), (0x6F, "Num /"),
    (0xBD, "-"), (0xBB, "="), (0xDB, "["), (0xDD, "]"), (0xBA, ";"), (0xDE, "'"),
    (0xBC, ","), (0xBE, "."), (0xBF, "/"), (0xDC, "\\"), (0xC0, "`"),
];

pub fn key_name(vk: u16) -> String {
    if vk == 0 {
        return "Не выбрана".to_owned();
    }
    KEYS.iter()
        .find(|(code, _)| *code == vk)
        .map(|(_, name)| (*name).to_owned())
        .unwrap_or_else(|| format!("VK 0x{vk:02X}"))
}

fn default_deadzone() -> f32 {
    0.12
}

fn default_saturation() -> f32 {
    1.0
}

fn default_sharpness() -> f32 {
    1.0
}

impl MappingEntry {
    /// Назначение читает всю ось (стик или триггер «на всю ось»), а не её половину.
    /// Триггеры и клавиши считают ход оси от покоя в одну сторону (половина),
    /// стики — всю ось.
    pub fn uses_full_axis(&self) -> bool {
        self.control.is_axis()
    }

    pub fn new(control: OutputControl, binding: InputBinding) -> Self {
        Self {
            control,
            binding,
            invert: matches!(
                control,
                OutputControl::LeftStickY | OutputControl::RightStickY
            ),
            centered_axis: false,
            deadzone: default_deadzone(),
            saturation: default_saturation(),
            sharpness: default_sharpness(),
            trigger: TriggerType::Direct,
        }
    }

    /// Мёртвая зона, максимум и резкость — по умолчанию.
    pub fn reset_axis_settings(&mut self) {
        self.deadzone = default_deadzone();
        self.saturation = default_saturation();
        self.sharpness = default_sharpness();
    }
}

/// Ось, которую двигают и педаль, и лепесток: быстрый скачок до упора —
/// лепесток, плавное движение — педаль.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaddleAxis {
    pub axis: u32,
    /// Минимальный скачок (доля хода) за окно, чтобы считать его лепестком.
    #[serde(default = "default_paddle_min_jump")]
    pub min_jump: f32,
    /// Окно скачка, мс.
    #[serde(default = "default_paddle_window_ms")]
    pub window_ms: u32,
}

impl PaddleAxis {
    pub fn new(axis: u32) -> Self {
        Self {
            axis,
            min_jump: default_paddle_min_jump(),
            window_ms: default_paddle_window_ms(),
        }
    }
}

fn default_paddle_min_jump() -> f32 {
    0.35
}

fn default_paddle_window_ms() -> u32 {
    25
}

/// Максимум кнопок-модификаторов (слоёв) в профиле.
pub const MAX_MODIFIERS: usize = 4;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControllerProfile {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub device_guid: String,
    #[serde(default)]
    pub entries: Vec<MappingEntry>,
    #[serde(default)]
    pub modifiers: Vec<ModifierConfig>,
    #[serde(default)]
    pub keyboard: Vec<KeyboardMapping>,
    /// Кнопки, двигающие мышь (обзор камеры).
    #[serde(default)]
    pub mouse: Vec<MouseMapping>,
    #[serde(default)]
    pub timings: Timings,
    #[serde(default)]
    pub paddle_axes: Vec<PaddleAxis>,
    /// Положение покоя осей (педали отпущены, руль по центру); запоминается
    /// при назначении оси и кнопкой «Запомнить покой осей».
    #[serde(default)]
    pub axis_rest: Vec<Option<i16>>,
}

impl ControllerProfile {
    pub fn default_for(device_guid: String) -> Self {
        let entries = vec![
            MappingEntry::new(OutputControl::A, InputBinding::Button { index: 0 }),
            MappingEntry::new(OutputControl::B, InputBinding::Button { index: 1 }),
            MappingEntry::new(OutputControl::X, InputBinding::Button { index: 2 }),
            MappingEntry::new(OutputControl::Y, InputBinding::Button { index: 3 }),
            MappingEntry::new(
                OutputControl::LeftShoulder,
                InputBinding::Button { index: 4 },
            ),
            MappingEntry::new(
                OutputControl::RightShoulder,
                InputBinding::Button { index: 5 },
            ),
            MappingEntry::new(OutputControl::Back, InputBinding::Button { index: 6 }),
            MappingEntry::new(OutputControl::Start, InputBinding::Button { index: 7 }),
            MappingEntry::new(OutputControl::Guide, InputBinding::Button { index: 8 }),
            MappingEntry::new(OutputControl::LeftThumb, InputBinding::Button { index: 9 }),
            MappingEntry::new(
                OutputControl::RightThumb,
                InputBinding::Button { index: 10 },
            ),
            MappingEntry::new(
                OutputControl::DpadUp,
                InputBinding::Hat {
                    index: 0,
                    direction: HatDirection::Up,
                },
            ),
            MappingEntry::new(
                OutputControl::DpadRight,
                InputBinding::Hat {
                    index: 0,
                    direction: HatDirection::Right,
                },
            ),
            MappingEntry::new(
                OutputControl::DpadDown,
                InputBinding::Hat {
                    index: 0,
                    direction: HatDirection::Down,
                },
            ),
            MappingEntry::new(
                OutputControl::DpadLeft,
                InputBinding::Hat {
                    index: 0,
                    direction: HatDirection::Left,
                },
            ),
            MappingEntry::new(
                OutputControl::LeftTrigger,
                InputBinding::AxisPositive { index: 4 },
            ),
            MappingEntry::new(
                OutputControl::RightTrigger,
                InputBinding::AxisPositive { index: 5 },
            ),
            MappingEntry::new(
                OutputControl::LeftStickX,
                InputBinding::AxisPositive { index: 0 },
            ),
            MappingEntry::new(
                OutputControl::LeftStickY,
                InputBinding::AxisPositive { index: 1 },
            ),
            MappingEntry::new(
                OutputControl::RightStickX,
                InputBinding::AxisPositive { index: 2 },
            ),
            MappingEntry::new(
                OutputControl::RightStickY,
                InputBinding::AxisPositive { index: 3 },
            ),
        ];

        Self {
            name: "Профиль по умолчанию".to_owned(),
            device_guid,
            entries,
            modifiers: Vec::new(),
            keyboard: Vec::new(),
            mouse: Vec::new(),
            timings: Timings::default(),
            paddle_axes: Vec::new(),
            axis_rest: Vec::new(),
        }
    }

    /// Запомнить покой только тех осей, для которых он ещё не известен.
    pub fn fill_missing_rest(&mut self, axes: &[i16]) {
        if self.axis_rest.len() < axes.len() {
            self.axis_rest.resize(axes.len(), None);
        }
        for (rest, value) in self.axis_rest.iter_mut().zip(axes) {
            rest.get_or_insert(*value);
        }
    }

    /// Запомнить покой оси `index` из `baseline` (None — всех осей);
    /// для неизвестных осей — тоже.
    pub fn remember_rest(&mut self, baseline: &[i16], index: Option<usize>) {
        if self.axis_rest.len() < baseline.len() {
            self.axis_rest.resize(baseline.len(), None);
        }
        for (axis, value) in baseline.iter().enumerate() {
            if self.axis_rest[axis].is_none() || Some(axis) == index || index.is_none() {
                self.axis_rest[axis] = Some(*value);
            }
        }
    }

    /// Чистый профиль: ни одного назначения, модификатора, клавиши и лепестка.
    pub fn empty(name: String, device_guid: String) -> Self {
        let mut profile = Self::default_for(device_guid);
        profile.name = name;
        for entry in &mut profile.entries {
            entry.binding = InputBinding::None;
        }
        profile
    }

    pub fn entry(&self, control: OutputControl) -> Option<&MappingEntry> {
        self.entries.iter().find(|entry| entry.control == control)
    }

    pub fn entry_mut(&mut self, control: OutputControl) -> &mut MappingEntry {
        entry_mut_in(&mut self.entries, control)
    }

    /// Назначения Xbox слоя: None — основной, Some(n) — модификатор n.
    pub fn layer_entries(&self, layer: Option<usize>) -> &[MappingEntry] {
        match layer {
            None => &self.entries,
            Some(index) => self
                .modifiers
                .get(index)
                .map(|modifier| modifier.entries.as_slice())
                .unwrap_or(&[]),
        }
    }

    /// Приводит загруженный из файла профиль в рабочее состояние.
    pub fn sanitize(&mut self) {
        self.name = self.name.trim().to_owned();
        self.modifiers.truncate(MAX_MODIFIERS);
        let layers = self.modifiers.len();
        self.keyboard
            .retain(|mapping| mapping.modifier.is_none_or(|layer| layer < layers));
        self.mouse
            .retain(|mapping| mapping.modifier.is_none_or(|layer| layer < layers));
        for mapping in &mut self.mouse {
            mapping.speed = mapping.speed.clamp(1, 100);
        }
        let mut seen = Vec::new();
        self.entries.retain(|entry| {
            let fresh = !seen.contains(&entry.control);
            seen.push(entry.control);
            fresh
        });
        // Газ и тормоз на противоположных половинах одной оси, но «на всю ось»:
        // в покое оба триггера были бы нажаты наполовину. Это общая ось педалей.
        let axis = |control| match self.entry(control).map(|entry| entry.binding) {
            Some(InputBinding::AxisPositive { index }) => Some((index, false)),
            Some(InputBinding::AxisNegative { index }) => Some((index, true)),
            _ => Option::None,
        };
        if let (Some((left, left_negative)), Some((right, right_negative))) =
            (axis(OutputControl::LeftTrigger), axis(OutputControl::RightTrigger))
        {
            if left == right && left_negative != right_negative {
                self.entry_mut(OutputControl::LeftTrigger).centered_axis = true;
                self.entry_mut(OutputControl::RightTrigger).centered_axis = true;
            }
        }
    }

    /// Удаляет модификатор и сдвигает ссылки на последующие слои.
    pub fn remove_modifier(&mut self, index: usize) {
        if index >= self.modifiers.len() {
            return;
        }
        self.modifiers.remove(index);
        self.keyboard
            .retain(|mapping| mapping.modifier != Some(index));
        for mapping in &mut self.keyboard {
            if let Some(layer) = mapping.modifier.as_mut() {
                if *layer > index {
                    *layer -= 1;
                }
            }
        }
        self.mouse.retain(|mapping| mapping.modifier != Some(index));
        for mapping in &mut self.mouse {
            if let Some(layer) = mapping.modifier.as_mut() {
                if *layer > index {
                    *layer -= 1;
                }
            }
        }
    }

    /// Назначение мыши слоя `layer` в направлении `direction`.
    pub fn mouse_mapping(&self, layer: Option<usize>, direction: MouseDirection) -> Option<&MouseMapping> {
        self.mouse
            .iter()
            .find(|mapping| mapping.modifier == layer && mapping.direction == direction)
    }

    /// То же, с созданием пустого назначения, если его нет.
    pub fn mouse_mapping_mut(&mut self, layer: Option<usize>, direction: MouseDirection) -> &mut MouseMapping {
        if let Some(index) = self
            .mouse
            .iter()
            .position(|mapping| mapping.modifier == layer && mapping.direction == direction)
        {
            return &mut self.mouse[index];
        }
        self.mouse.push(MouseMapping {
            input: InputBinding::None,
            modifier: layer,
            direction,
            speed: default_mouse_speed(),
        });
        self.mouse.last_mut().expect("mouse mapping was inserted")
    }
}

pub fn entry_mut_in(entries: &mut Vec<MappingEntry>, control: OutputControl) -> &mut MappingEntry {
    if let Some(index) = entries.iter().position(|entry| entry.control == control) {
        return &mut entries[index];
    }
    entries.push(MappingEntry::new(control, InputBinding::None));
    entries.last_mut().expect("mapping entry was inserted")
}

/// Одно физическое действие включает оба входа. `full_*` — назначение
/// читает всю ось: тогда оно пересекается с любой половиной той же оси.
/// Газ на «Ось 4 −» и тормоз на «Ось 4 +» (половины) не пересекаются.
pub fn inputs_overlap(a: InputBinding, full_a: bool, b: InputBinding, full_b: bool) -> bool {
    use InputBinding::*;
    let axis = |binding| match binding {
        AxisPositive { index } => Some((index, false)),
        AxisNegative { index } => Some((index, true)),
        _ => Option::None,
    };
    match (a, b) {
        (None, _) | (_, None) => false,
        (Hat { index: i, direction: x }, Hat { index: j, direction: y }) => {
            let parts = |d: HatDirection| {
                [HatDirection::Up, HatDirection::Right, HatDirection::Down, HatDirection::Left]
                    .map(|cardinal| d.contains(cardinal))
            };
            let (x, y) = (parts(x), parts(y));
            i == j && (0..4).any(|k| x[k] && y[k])
        }
        _ => match (axis(a), axis(b)) {
            (Some((i, neg_a)), Some((j, neg_b))) => i == j && (full_a || full_b || neg_a == neg_b),
            _ => a == b,
        },
    }
}

#[derive(Clone, Debug, Default)]
pub struct RuntimeSnapshot {
    pub devices: Vec<DeviceDescriptor>,
    pub selected_instance_id: Option<u32>,
    pub raw_state: RawState,
    pub virtual_connected: bool,
    pub forwarding_active: bool,
    pub driver_installed: bool,
    pub last_error: Option<String>,
    pub last_controller_activity_sequence: u64,
}

/// Тот же физический контроллер, хотя GUID SDL другой. SDL меняет в GUID
/// контрольную сумму имени и байт драйвера (RawInput/HIDAPI/DirectInput)
/// от запуска к запуску, а VID и PID (символы 8..12 и 16..20) остаются.
pub fn same_device(a: &str, b: &str) -> bool {
    if a.eq_ignore_ascii_case(b) {
        return true;
    }
    let ids = |guid: &str| {
        let guid = guid.to_ascii_lowercase();
        (guid.len() == 32 && guid.is_ascii())
            .then(|| (guid[8..12].to_owned(), guid[16..20].to_owned()))
            .filter(|(vendor, _)| vendor != "0000")
    };
    matches!((ids(a), ids(b)), (Some(x), Some(y)) if x == y)
}

#[cfg(test)]
mod sanitize_tests {
    use super::*;

    #[test]
    fn shared_pedal_axis_becomes_half_axes_and_bad_layers_are_dropped() {
        let mut profile = ControllerProfile::default_for(String::new());
        profile.entry_mut(OutputControl::RightTrigger).binding = InputBinding::AxisNegative { index: 3 };
        profile.entry_mut(OutputControl::LeftTrigger).binding = InputBinding::AxisPositive { index: 3 };
        profile.keyboard.push(KeyboardMapping { modifier: Some(2), ..Default::default() });
        profile.sanitize();
        assert!(profile.entry(OutputControl::RightTrigger).unwrap().centered_axis);
        assert!(profile.entry(OutputControl::LeftTrigger).unwrap().centered_axis);
        assert!(profile.keyboard.is_empty());
        // Отдельные педали на разных осях не трогаем.
        let mut separate = ControllerProfile::default_for(String::new());
        separate.sanitize();
        assert!(!separate.entry(OutputControl::LeftTrigger).unwrap().centered_axis);
    }

    #[test]
    fn mouse_mappings_load_save_and_follow_modifier_removal() {
        let old: ControllerProfile = serde_json::from_str(r#"{"name":"old"}"#).unwrap();
        assert!(old.mouse.is_empty(), "старый профиль без мыши");

        let mut profile = ControllerProfile::default_for(String::new());
        profile.modifiers = vec![ModifierConfig::default(), ModifierConfig::default()];
        profile.mouse_mapping_mut(Some(1), MouseDirection::Up).input = InputBinding::Button { index: 3 };
        profile.mouse_mapping_mut(Some(0), MouseDirection::Left).speed = 0;
        let json = serde_json::to_string(&profile).unwrap();
        let mut loaded: ControllerProfile = serde_json::from_str(&json).unwrap();
        loaded.sanitize();
        assert_eq!(loaded.mouse_mapping(Some(0), MouseDirection::Left).unwrap().speed, 1, "скорость не ниже 1 %");
        assert_eq!(loaded.mouse_mapping(None, MouseDirection::Up), None);

        loaded.remove_modifier(0);
        assert_eq!(loaded.mouse.len(), 1);
        let up = loaded.mouse_mapping(Some(0), MouseDirection::Up).unwrap();
        assert_eq!((up.input, up.speed), (InputBinding::Button { index: 3 }, default_mouse_speed()));
    }
}

#[cfg(test)]
mod same_device_tests {
    use super::same_device;

    #[test]
    fn same_vid_pid_with_other_crc_and_driver_is_same_device() {
        let raw = "03008d3a6d0400006dc2000000007200";
        let dinput = "030000006d0400006dc2000000000000";
        assert!(same_device(raw, dinput));
        assert!(!same_device(raw, "030000006d0400001bc2000000000000"));
        // GUID без VID/PID сравниваем только целиком.
        assert!(!same_device(
            "05000000000000000000000000000000",
            "05000000000000000000000000000001"
        ));
    }
}

#[cfg(test)]
mod names_tests {
    use super::*;

    #[test]
    fn old_profiles_without_names_still_load_and_names_round_trip() {
        let old: ModifierConfig = serde_json::from_str(r#"{"mode":"layer-only"}"#).unwrap();
        assert!(old.name.is_empty());
        let old: KeyboardMapping = serde_json::from_str(r#"{"modifier":1}"#).unwrap();
        assert!(old.action.is_empty());

        let mapping = KeyboardMapping { action: "Шасси".to_owned(), ..Default::default() };
        let back: KeyboardMapping =
            serde_json::from_str(&serde_json::to_string(&mapping).unwrap()).unwrap();
        assert_eq!(back.action, "Шасси");
    }
}
