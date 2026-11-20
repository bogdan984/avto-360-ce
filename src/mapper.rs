use crate::model::{
    inputs_overlap, ControllerProfile, HatDirection, InputBinding, KeyOutput, MappingEntry,
    ModifierMode, OutputControl, PressMode, RawState, TriggerType,
};
use std::time::{Duration, Instant};
use vigem_rust::{X360Button, X360Report};

const LEARN_AXIS_DELTA_THRESHOLD: i32 = 7_500;
const LEARN_AXIS_ABSOLUTE_THRESHOLD: i32 = 12_000;
const BUTTON_AXIS_THRESHOLD: f32 = 0.55;

pub fn detect_binding(previous: &RawState, current: &RawState) -> Option<InputBinding> {
    for (index, pressed) in current.buttons.iter().copied().enumerate() {
        let was_pressed = previous.buttons.get(index).copied().unwrap_or(false);
        if pressed && !was_pressed {
            return Some(InputBinding::Button {
                index: index as u32,
            });
        }
    }

    for (axis, negative) in current.paddles.iter().copied() {
        if !previous.paddles.contains(&(axis, negative)) {
            return Some(InputBinding::Paddle { axis, negative });
        }
    }

    for (index, direction) in current.hats.iter().copied().enumerate() {
        let previous_direction = previous
            .hats
            .get(index)
            .copied()
            .unwrap_or(HatDirection::Centered);
        if direction != HatDirection::Centered && direction != previous_direction {
            return Some(InputBinding::Hat {
                index: index as u32,
                direction,
            });
        }
    }

    let mut best_axis: Option<(i32, InputBinding)> = None;
    for (index, value) in current.axes.iter().copied().enumerate() {
        let previous_value = previous.axes.get(index).copied().unwrap_or_default() as i32;
        let current_value = value as i32;
        let delta = current_value - previous_value;
        let magnitude = current_value.abs();

        let crossed_with_force = delta.abs() >= LEARN_AXIS_DELTA_THRESHOLD
            && (magnitude >= LEARN_AXIS_ABSOLUTE_THRESHOLD
                || delta.abs() >= LEARN_AXIS_DELTA_THRESHOLD * 2);
        if !crossed_with_force {
            continue;
        }

        let candidate = if delta >= 0 {
            InputBinding::AxisPositive {
                index: index as u32,
            }
        } else {
            InputBinding::AxisNegative {
                index: index as u32,
            }
        };
        let score = delta.abs();
        if best_axis
            .as_ref()
            .map(|(best_score, _)| score > *best_score)
            .unwrap_or(true)
        {
            best_axis = Some((score, candidate));
        }
    }

    best_axis.map(|(_, binding)| binding)
}

/// Прямые (Direct) назначения основного слоя. Привязки из `suppressed`
/// (захвачены слоем модификатора) и назначения с временным типом нажатия
/// пропускаются — их обрабатывает [`MapperState`].
fn map_direct_report(
    profile: &ControllerProfile,
    raw: &RawState,
    suppressed: &[InputBinding],
) -> X360Report {
    let mut report = X360Report::default();
    let entry = |control: OutputControl| {
        profile.entry(control).filter(|entry| {
            !suppressed.contains(&entry.binding)
                && (control.is_axis() || entry.trigger == TriggerType::Direct)
                && !keyboard_owns(profile, None, entry)
        })
    };

    for control in OutputControl::BUTTONS {
        // Never expose the virtual Guide/Nexus button to Windows. Windows can
        // translate it into an ms-gamebar activation even when Game Bar is not
        // installed, which produces a disruptive protocol-handler dialog.
        if control == OutputControl::Guide {
            continue;
        }

        let Some(entry) = entry(control) else {
            continue;
        };
        if binding_pressed(entry, raw) {
            report.buttons.insert(button_flag(control));
        }
    }

    report.left_trigger = entry(OutputControl::LeftTrigger)
        .map(|entry| trigger_value(entry, raw))
        .unwrap_or_default();
    report.right_trigger = entry(OutputControl::RightTrigger)
        .map(|entry| trigger_value(entry, raw))
        .unwrap_or_default();
    report.thumb_lx = entry(OutputControl::LeftStickX)
        .map(|entry| axis_value(entry, raw))
        .unwrap_or_default();
    report.thumb_ly = entry(OutputControl::LeftStickY)
        .map(|entry| axis_value(entry, raw))
        .unwrap_or_default();
    report.thumb_rx = entry(OutputControl::RightStickX)
        .map(|entry| axis_value(entry, raw))
        .unwrap_or_default();
    report.thumb_ry = entry(OutputControl::RightStickY)
        .map(|entry| axis_value(entry, raw))
        .unwrap_or_default();

    report
}

/// Длительность импульса (Pulse) для клавиш и кнопок Xbox.
pub const PULSE: Duration = Duration::from_millis(40);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Xbox(OutputControl),
    Key(KeyOutput),
}

impl Action {
    fn hold_allowed(self) -> bool {
        match self {
            Self::Xbox(_) => true,
            Self::Key(key) => key.mode == PressMode::HoldWhilePressed,
        }
    }
}

#[derive(Default)]
struct RuleSet {
    direct: Vec<Action>,
    single: Vec<Action>,
    double: Vec<Action>,
    long: Vec<Action>,
}

impl RuleSet {
    fn is_empty(&self) -> bool {
        self.direct.is_empty()
            && self.single.is_empty()
            && self.double.is_empty()
            && self.long.is_empty()
    }

    fn push(&mut self, trigger: TriggerType, action: Action) {
        match trigger {
            TriggerType::Direct => self.direct.push(action),
            TriggerType::SingleTap => self.single.push(action),
            TriggerType::DoubleTap => self.double.push(action),
            TriggerType::LongPress => self.long.push(action),
        }
    }

    fn all(self) -> Vec<Action> {
        let mut all = self.direct;
        all.extend(self.single);
        all.extend(self.double);
        all.extend(self.long);
        all
    }
}

/// Правила для физического входа в слое. Прямые назначения Xbox основного
/// слоя добавляются только при `with_base_direct` (их обычно выдаёт
/// [`map_direct_report`] без задержки).
fn rules_for(
    profile: &ControllerProfile,
    binding: InputBinding,
    layer: Option<usize>,
    with_base_direct: bool,
) -> RuleSet {
    let mut rules = RuleSet::default();
    for entry in profile.layer_entries(layer) {
        if entry.binding != binding
            || entry.control.is_axis()
            || keyboard_owns(profile, layer, entry)
        {
            continue;
        }
        if layer.is_none() && entry.trigger == TriggerType::Direct && !with_base_direct {
            continue;
        }
        rules.push(entry.trigger, Action::Xbox(entry.control));
    }
    for mapping in &profile.keyboard {
        if mapping.input == binding && mapping.modifier == layer && mapping.key.vk != 0 {
            rules.push(mapping.trigger, Action::Key(mapping.key));
        }
    }
    rules
}

/// Вход назначен на клавишу в этом слое — тогда назначение Xbox на тот же
/// вход не работает: один вход либо джойстик, либо клавиатура, не оба сразу.
pub fn keyboard_owns(profile: &ControllerProfile, layer: Option<usize>, entry: &MappingEntry) -> bool {
    profile.keyboard.iter().any(|mapping| {
        mapping.modifier == layer
            && mapping.key.vk != 0
            && inputs_overlap(mapping.input, false, entry.binding, entry.uses_full_axis())
    })
}

/// Состояние одного отслеживаемого физического входа.
struct Track {
    binding: InputBinding,
    pressed: bool,
    /// Слой, выбранный в момент нажатия; держится до полного отпускания.
    layer: Option<usize>,
    pressed_at: Instant,
    released_at: Instant,
    /// Было короткое нажатие, ждём второе (окно двойного нажатия).
    waiting_double: bool,
    /// Текущее нажатие уже отработало временное действие.
    consumed: bool,
    /// Действия, активные пока вход зажат.
    held: Vec<Action>,
    /// Для модификатора: во время удержания нажимали другую кнопку.
    used: bool,
}

impl Track {
    fn new(binding: InputBinding, now: Instant) -> Self {
        Self {
            binding,
            pressed: false,
            layer: None,
            pressed_at: now,
            released_at: now,
            waiting_double: false,
            consumed: false,
            held: Vec::new(),
            used: false,
        }
    }
}

#[derive(Default)]
pub struct MapperOutput {
    pub report: X360Report,
    /// Клавиши, которые должны быть зажаты прямо сейчас.
    pub keys: Vec<KeyOutput>,
}

/// Конечный автомат нажатий: модификаторы, одиночное/двойное/долгое
/// нажатие и клавиатура. Вызывается на каждом опросе контроллера.
#[derive(Default)]
pub struct MapperState {
    tracks: Vec<Track>,
    pulses: Vec<(Action, Instant)>,
    previous: RawState,
}

impl MapperState {
    /// Сбросить всё: все кнопки и клавиши будут отпущены.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn update(
        &mut self,
        profile: &ControllerProfile,
        raw: &RawState,
        now: Instant,
    ) -> MapperOutput {
        self.sync_tracks(profile, now);
        self.pulses.retain(|(_, until)| *until > now);
        self.mark_used_modifiers(profile, raw);

        let window = Duration::from_millis(u64::from(profile.timings.double_tap_window_ms));
        let hold = Duration::from_millis(u64::from(profile.timings.hold_threshold_ms));
        let mut pulses = Vec::new();
        let mut used_layers = Vec::new();

        for track in &mut self.tracks {
            let pressed = input_pressed(track.binding, raw);
            let modifier = profile
                .modifiers
                .iter()
                .find(|modifier| modifier.binding == track.binding)
                .map(|modifier| modifier.mode);

            match modifier {
                Some(ModifierMode::LayerOnly) => {
                    track.pressed = pressed;
                    continue;
                }
                Some(ModifierMode::TapAction) => {
                    if pressed && !track.pressed {
                        track.used = false;
                    } else if !pressed && track.pressed && !track.used {
                        let actions = rules_for(profile, track.binding, None, true).all();
                        pulses.extend(actions);
                    }
                    track.pressed = pressed;
                    continue;
                }
                Some(ModifierMode::Passthrough) | None => {}
            }

            if pressed && !track.pressed {
                // Нажатие: выбираем слой по зажатым модификаторам.
                let previous_layer = track.layer;
                track.layer = if modifier.is_some() {
                    None
                } else {
                    active_layer(profile, track.binding, raw)
                };
                if let Some(layer) = track.layer {
                    used_layers.push(layer);
                }
                track.pressed = true;
                track.pressed_at = now;
                track.consumed = false;
                track.held.clear();
                let rules = rules_for(profile, track.binding, track.layer, false);

                let double_ready = track.waiting_double
                    && track.layer == previous_layer
                    && !rules.double.is_empty()
                    && now.duration_since(track.released_at) <= window;
                if track.waiting_double && !double_ready {
                    // Ожидавшее одиночное нажатие не должно потеряться.
                    pulses.extend(rules_for(profile, track.binding, previous_layer, false).single);
                }
                track.waiting_double = false;
                fire(track, &mut pulses, &rules.direct);
                if double_ready {
                    fire(track, &mut pulses, &rules.double);
                    track.consumed = true;
                } else if !rules.single.is_empty()
                    && rules.double.is_empty()
                    && rules.long.is_empty()
                {
                    // Конфликтов нет — одиночное без задержки.
                    fire(track, &mut pulses, &rules.single);
                    track.consumed = true;
                }
            } else if pressed {
                if !track.consumed && now.duration_since(track.pressed_at) >= hold {
                    let rules = rules_for(profile, track.binding, track.layer, false);
                    if !rules.long.is_empty() {
                        fire(track, &mut pulses, &rules.long);
                        track.consumed = true;
                    }
                }
            } else if track.pressed {
                // Отпускание.
                track.pressed = false;
                track.released_at = now;
                track.held.clear();
                if !track.consumed {
                    let rules = rules_for(profile, track.binding, track.layer, false);
                    if !rules.double.is_empty() {
                        track.waiting_double = true;
                    } else {
                        pulses.extend(rules.single);
                    }
                }
                track.consumed = false;
            } else if track.waiting_double && now.duration_since(track.released_at) > window {
                // Второго нажатия не было — это одиночное.
                track.waiting_double = false;
                pulses.extend(rules_for(profile, track.binding, track.layer, false).single);
            }
        }

        for layer in used_layers {
            if let Some(binding) = profile
                .modifiers
                .get(layer)
                .map(|modifier| modifier.binding)
            {
                for track in self
                    .tracks
                    .iter_mut()
                    .filter(|track| track.binding == binding)
                {
                    track.used = true;
                }
            }
        }
        self.pulses
            .extend(pulses.into_iter().map(|action| (action, now + PULSE)));
        self.previous = raw.clone();
        self.output(profile, raw)
    }

    fn output(&self, profile: &ControllerProfile, raw: &RawState) -> MapperOutput {
        let mut suppressed: Vec<InputBinding> = profile
            .modifiers
            .iter()
            .filter(|modifier| modifier.mode != ModifierMode::Passthrough)
            .map(|modifier| modifier.binding)
            .collect();
        suppressed.extend(
            self.tracks
                .iter()
                .filter(|track| track.pressed && track.layer.is_some())
                .map(|track| track.binding),
        );

        let mut output = MapperOutput {
            report: map_direct_report(profile, raw, &suppressed),
            keys: Vec::new(),
        };
        let held = self
            .tracks
            .iter()
            .flat_map(|track| track.held.iter().copied());
        let pulsed = self.pulses.iter().map(|(action, _)| *action);
        for action in held.chain(pulsed) {
            match action {
                Action::Xbox(OutputControl::Guide) => {}
                Action::Xbox(OutputControl::LeftTrigger) => output.report.left_trigger = u8::MAX,
                Action::Xbox(OutputControl::RightTrigger) => output.report.right_trigger = u8::MAX,
                Action::Xbox(control) => output.report.buttons.insert(button_flag(control)),
                Action::Key(key) => {
                    if !output.keys.contains(&key) {
                        output.keys.push(key);
                    }
                }
            }
        }
        output
    }

    /// Держит список отслеживаемых входов в соответствии с профилем.
    fn sync_tracks(&mut self, profile: &ControllerProfile, now: Instant) {
        let mut wanted: Vec<InputBinding> = Vec::new();
        let mut add = |binding: InputBinding| {
            if binding != InputBinding::None && !wanted.contains(&binding) {
                wanted.push(binding);
            }
        };
        for modifier in &profile.modifiers {
            add(modifier.binding);
            for entry in &modifier.entries {
                add(entry.binding);
            }
        }
        for mapping in &profile.keyboard {
            add(mapping.input);
        }
        for entry in &profile.entries {
            if entry.trigger != TriggerType::Direct && !entry.control.is_axis() {
                add(entry.binding);
            }
        }

        self.tracks.retain(|track| wanted.contains(&track.binding));
        for binding in wanted {
            if !self.tracks.iter().any(|track| track.binding == binding) {
                self.tracks.push(Track::new(binding, now));
            }
        }
    }

    /// Модификатор «использован», если пока он зажат нажали другую кнопку.
    fn mark_used_modifiers(&mut self, profile: &ControllerProfile, raw: &RawState) {
        let newly_pressed: Vec<InputBinding> = raw
            .buttons
            .iter()
            .enumerate()
            .filter(|(index, pressed)| {
                **pressed && !self.previous.buttons.get(*index).copied().unwrap_or(false)
            })
            .map(|(index, _)| InputBinding::Button {
                index: index as u32,
            })
            .chain(
                raw.hats
                    .iter()
                    .enumerate()
                    .filter(|(index, direction)| {
                        **direction != HatDirection::Centered
                            && self.previous.hats.get(*index) != Some(*direction)
                    })
                    .map(|(index, direction)| InputBinding::Hat {
                        index: index as u32,
                        direction: *direction,
                    }),
            )
            .chain(
                raw.paddles
                    .iter()
                    .filter(|paddle| !self.previous.paddles.contains(paddle))
                    .map(|(axis, negative)| InputBinding::Paddle {
                        axis: *axis,
                        negative: *negative,
                    }),
            )
            .collect();
        for modifier in &profile.modifiers {
            let other_pressed = newly_pressed
                .iter()
                .any(|binding| !same_physical_input(*binding, modifier.binding));
            if !other_pressed {
                continue;
            }
            for track in self
                .tracks
                .iter_mut()
                .filter(|track| track.binding == modifier.binding && track.pressed)
            {
                track.used = true;
            }
        }
    }
}

/// Срабатывание во время нажатия: удерживаемые действия живут до отпускания,
/// импульсные — [`PULSE`].
fn fire(track: &mut Track, pulses: &mut Vec<Action>, actions: &[Action]) {
    for action in actions {
        if action.hold_allowed() {
            track.held.push(*action);
        } else {
            pulses.push(*action);
        }
    }
}

/// Первый зажатый модификатор, в слое которого у входа есть правила.
fn active_layer(
    profile: &ControllerProfile,
    binding: InputBinding,
    raw: &RawState,
) -> Option<usize> {
    profile
        .modifiers
        .iter()
        .enumerate()
        .find_map(|(index, modifier)| {
            let held = modifier.binding != InputBinding::None
                && !same_physical_input(modifier.binding, binding)
                && input_pressed(modifier.binding, raw);
            (held && !rules_for(profile, binding, Some(index), true).is_empty()).then_some(index)
        })
}

fn same_physical_input(a: InputBinding, b: InputBinding) -> bool {
    match (a, b) {
        (InputBinding::Hat { index: left, .. }, InputBinding::Hat { index: right, .. }) => {
            left == right
        }
        _ => a == b,
    }
}

pub fn binding_pressed(entry: &MappingEntry, raw: &RawState) -> bool {
    input_pressed(entry.binding, raw)
}

pub fn input_pressed(binding: InputBinding, raw: &RawState) -> bool {
    match binding {
        InputBinding::None => false,
        InputBinding::Button { index } => raw.buttons.get(index as usize).copied().unwrap_or(false),
        InputBinding::AxisPositive { index } => normalized_axis(raw, index) > BUTTON_AXIS_THRESHOLD,
        InputBinding::AxisNegative { index } => {
            normalized_axis(raw, index) < -BUTTON_AXIS_THRESHOLD
        }
        InputBinding::Hat { index, direction } => raw
            .hats
            .get(index as usize)
            .copied()
            .unwrap_or(HatDirection::Centered)
            .contains(direction),
        InputBinding::Paddle { axis, negative } => raw.paddles.contains(&(axis, negative)),
    }
}

pub fn axis_preview(entry: &MappingEntry, raw: &RawState) -> f32 {
    let mut value = match entry.binding {
        InputBinding::None => 0.0,
        InputBinding::Button { index } => {
            if raw.buttons.get(index as usize).copied().unwrap_or(false) {
                1.0
            } else {
                0.0
            }
        }
        InputBinding::Paddle { .. } => {
            if input_pressed(entry.binding, raw) {
                1.0
            } else {
                0.0
            }
        }
        InputBinding::AxisPositive { index } => normalized_axis(raw, index),
        InputBinding::AxisNegative { index } => -normalized_axis(raw, index),
        InputBinding::Hat { index, direction } => {
            if raw
                .hats
                .get(index as usize)
                .copied()
                .unwrap_or(HatDirection::Centered)
                .contains(direction)
            {
                1.0
            } else {
                0.0
            }
        }
    };

    if entry.invert {
        value = -value;
    }

    apply_deadzone(value, entry.deadzone, entry.saturation)
}

fn axis_value(entry: &MappingEntry, raw: &RawState) -> i16 {
    let value = axis_preview(entry, raw).clamp(-1.0, 1.0);
    if value < 0.0 {
        (value * 32_768.0).round().clamp(-32_768.0, 0.0) as i16
    } else {
        (value * 32_767.0).round().clamp(0.0, 32_767.0) as i16
    }
}

pub fn trigger_preview(entry: &MappingEntry, raw: &RawState) -> f32 {
    let mut value = match entry.binding {
        InputBinding::None => 0.0,
        InputBinding::Button { index } => {
            if raw.buttons.get(index as usize).copied().unwrap_or(false) {
                1.0
            } else {
                0.0
            }
        }
        InputBinding::Paddle { .. } => {
            if input_pressed(entry.binding, raw) {
                1.0
            } else {
                0.0
            }
        }
        InputBinding::AxisPositive { index } => {
            let value = normalized_axis(raw, index);
            if entry.centered_axis {
                value.max(0.0)
            } else {
                ((value + 1.0) * 0.5).clamp(0.0, 1.0)
            }
        }
        InputBinding::AxisNegative { index } => {
            let value = -normalized_axis(raw, index);
            if entry.centered_axis {
                value.max(0.0)
            } else {
                ((value + 1.0) * 0.5).clamp(0.0, 1.0)
            }
        }
        InputBinding::Hat { index, direction } => {
            if raw
                .hats
                .get(index as usize)
                .copied()
                .unwrap_or(HatDirection::Centered)
                .contains(direction)
            {
                1.0
            } else {
                0.0
            }
        }
    };

    if entry.invert {
        value = 1.0 - value;
    }
    apply_trigger_deadzone(value, entry.deadzone, entry.saturation)
}

fn trigger_value(entry: &MappingEntry, raw: &RawState) -> u8 {
    (trigger_preview(entry, raw) * 255.0)
        .round()
        .clamp(0.0, 255.0) as u8
}

fn normalized_axis(raw: &RawState, index: u32) -> f32 {
    let value = raw.axes.get(index as usize).copied().unwrap_or_default();
    if value < 0 {
        value as f32 / 32_768.0
    } else {
        value as f32 / 32_767.0
    }
}

fn apply_deadzone(value: f32, deadzone: f32, saturation: f32) -> f32 {
    let deadzone = deadzone.clamp(0.0, 0.95);
    let saturation = saturation.clamp(deadzone + 0.01, 1.0);
    let magnitude = value.abs();
    if magnitude <= deadzone {
        return 0.0;
    }
    let scaled = ((magnitude - deadzone) / (saturation - deadzone)).clamp(0.0, 1.0);
    scaled.copysign(value)
}

fn apply_trigger_deadzone(value: f32, deadzone: f32, saturation: f32) -> f32 {
    let deadzone = deadzone.clamp(0.0, 0.95);
    let saturation = saturation.clamp(deadzone + 0.01, 1.0);
    if value <= deadzone {
        0.0
    } else {
        ((value - deadzone) / (saturation - deadzone)).clamp(0.0, 1.0)
    }
}

fn button_flag(control: OutputControl) -> X360Button {
    match control {
        OutputControl::A => X360Button::A,
        OutputControl::B => X360Button::B,
        OutputControl::X => X360Button::X,
        OutputControl::Y => X360Button::Y,
        OutputControl::LeftShoulder => X360Button::LEFT_SHOULDER,
        OutputControl::RightShoulder => X360Button::RIGHT_SHOULDER,
        OutputControl::Back => X360Button::BACK,
        OutputControl::Start => X360Button::START,
        OutputControl::Guide => X360Button::GUIDE,
        OutputControl::LeftThumb => X360Button::LEFT_THUMB,
        OutputControl::RightThumb => X360Button::RIGHT_THUMB,
        OutputControl::DpadUp => X360Button::DPAD_UP,
        OutputControl::DpadRight => X360Button::DPAD_RIGHT,
        OutputControl::DpadDown => X360Button::DPAD_DOWN,
        OutputControl::DpadLeft => X360Button::DPAD_LEFT,
        _ => X360Button::empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learns_new_button_press() {
        let previous = RawState {
            buttons: vec![false],
            ..Default::default()
        };
        let current = RawState {
            buttons: vec![true],
            ..Default::default()
        };
        assert_eq!(
            detect_binding(&previous, &current),
            Some(InputBinding::Button { index: 0 })
        );
    }

    #[test]
    fn learns_paddle_and_maps_it_as_button() {
        let previous = RawState::default();
        let current = RawState {
            paddles: vec![(2, false)],
            ..Default::default()
        };
        let binding = detect_binding(&previous, &current).unwrap();
        assert_eq!(
            binding,
            InputBinding::Paddle {
                axis: 2,
                negative: false
            }
        );
        let mut profile = ControllerProfile::default_for(String::new());
        profile.entries = vec![MappingEntry::new(OutputControl::RightShoulder, binding)];
        let out = MapperState::default().update(&profile, &current, Instant::now());
        assert!(out.report.buttons.contains(X360Button::RIGHT_SHOULDER));
    }

    #[test]
    fn learns_axis_from_large_delta() {
        let previous = RawState {
            axes: vec![-32_768],
            ..Default::default()
        };
        let current = RawState {
            axes: vec![0],
            ..Default::default()
        };
        assert_eq!(
            detect_binding(&previous, &current),
            Some(InputBinding::AxisPositive { index: 0 })
        );
    }

    #[test]
    fn learns_negative_axis_direction() {
        let previous = RawState {
            axes: vec![0],
            ..Default::default()
        };
        let current = RawState {
            axes: vec![-20_000],
            ..Default::default()
        };
        assert_eq!(
            detect_binding(&previous, &current),
            Some(InputBinding::AxisNegative { index: 0 })
        );
    }

    #[test]
    fn deadzone_removes_center_noise() {
        assert_eq!(apply_deadzone(0.05, 0.12, 1.0), 0.0);
        assert!(apply_deadzone(0.75, 0.12, 1.0) > 0.6);
    }

    #[test]
    fn bipolar_trigger_is_zero_at_negative_rest() {
        let mut entry = MappingEntry::new(
            OutputControl::LeftTrigger,
            InputBinding::AxisPositive { index: 0 },
        );
        entry.deadzone = 0.0;
        let raw = RawState {
            axes: vec![i16::MIN],
            ..Default::default()
        };
        assert_eq!(trigger_preview(&entry, &raw), 0.0);
    }

    #[test]
    fn centered_trigger_uses_one_axis_half() {
        let mut entry = MappingEntry::new(
            OutputControl::LeftTrigger,
            InputBinding::AxisNegative { index: 0 },
        );
        entry.centered_axis = true;
        entry.deadzone = 0.0;
        let raw = RawState {
            axes: vec![-16_384],
            ..Default::default()
        };
        assert!(trigger_preview(&entry, &raw) > 0.49);
    }

    /// Газ и тормоз на одной оси: газ = «Ось 4 −», тормоз = «Ось 4 +».
    #[test]
    fn shared_pedal_axis_halves_do_not_conflict() {
        let mut profile = ControllerProfile::default_for(String::new());
        profile.entries.clear();
        for (control, binding) in [
            (OutputControl::RightTrigger, InputBinding::AxisNegative { index: 3 }),
            (OutputControl::LeftTrigger, InputBinding::AxisPositive { index: 3 }),
        ] {
            let mut entry = MappingEntry::new(control, binding);
            entry.centered_axis = true;
            profile.entries.push(entry);
        }
        let at = |value: i16| {
            let raw = RawState {
                axes: vec![0, 0, 0, value],
                ..Default::default()
            };
            let report = map_direct_report(&profile, &raw, &[]);
            (report.right_trigger, report.left_trigger)
        };
        assert_eq!(at(0), (0, 0));
        assert_eq!(at(-32_768), (255, 0));
        assert_eq!(at(32_767), (0, 255));
        // Сами назначения тоже считаются непересекающимися.
        let (gas, brake) = (&profile.entries[0], &profile.entries[1]);
        assert!(!inputs_overlap(gas.binding, gas.uses_full_axis(), brake.binding, brake.uses_full_axis()));
        // А стик на всей оси пересекается с обеими половинами.
        assert!(inputs_overlap(InputBinding::AxisPositive { index: 3 }, true, gas.binding, false));
    }

    #[test]
    fn keyboard_mapping_disables_xbox_on_same_input() {
        let mut profile = ControllerProfile::default_for(String::new());
        profile.entries = vec![MappingEntry::new(OutputControl::A, TARGET)];
        profile.keyboard = vec![keyboard(TARGET, None, TriggerType::Direct, key(0x45, PressMode::HoldWhilePressed))];
        let mut state = MapperState::default();
        let out = state.update(&profile, &buttons(&[1]), Instant::now());
        assert!(!out.report.buttons.contains(X360Button::A));
        assert_eq!(out.keys.len(), 1);
        // Без клавиши кнопка снова работает как Xbox A.
        profile.keyboard.clear();
        let out = MapperState::default().update(&profile, &buttons(&[1]), Instant::now());
        assert!(out.report.buttons.contains(X360Button::A));
    }

    use crate::model::{KeyboardMapping, ModifierConfig};

    const MODIFIER: InputBinding = InputBinding::Button { index: 0 };
    const TARGET: InputBinding = InputBinding::Button { index: 1 };

    fn buttons(pressed: &[usize]) -> RawState {
        RawState {
            buttons: (0..4).map(|index| pressed.contains(&index)).collect(),
            ..Default::default()
        }
    }

    fn key(vk: u16, mode: PressMode) -> KeyOutput {
        KeyOutput {
            vk,
            mode,
            ..Default::default()
        }
    }

    fn keyboard(
        input: InputBinding,
        modifier: Option<usize>,
        trigger: TriggerType,
        key: KeyOutput,
    ) -> KeyboardMapping {
        KeyboardMapping {
            input,
            modifier,
            trigger,
            key,
        }
    }

    /// Профиль без назначений, кроме: A ← кнопка 2 в основном слое,
    /// модификатор = кнопка 1, в его слое B ← кнопка 2.
    fn chord_profile(mode: ModifierMode) -> ControllerProfile {
        let mut profile = ControllerProfile::default_for(String::new());
        profile.entries = vec![MappingEntry::new(OutputControl::A, TARGET)];
        profile.modifiers = vec![ModifierConfig {
            binding: MODIFIER,
            mode,
            entries: vec![MappingEntry::new(OutputControl::B, TARGET)],
        }];
        profile
    }

    fn ms(start: Instant, millis: u64) -> Instant {
        start + Duration::from_millis(millis)
    }

    #[test]
    fn chord_fires_layer_action_and_blocks_base_action() {
        let profile = chord_profile(ModifierMode::LayerOnly);
        let mut state = MapperState::default();
        let t = Instant::now();

        let out = state.update(&profile, &buttons(&[0]), t);
        assert!(
            out.report.buttons.is_empty(),
            "модификатор сам ничего не жмёт"
        );

        let out = state.update(&profile, &buttons(&[0, 1]), ms(t, 10));
        assert!(out.report.buttons.contains(X360Button::B));
        assert!(!out.report.buttons.contains(X360Button::A));

        state.update(&profile, &buttons(&[]), ms(t, 20));
        let out = state.update(&profile, &buttons(&[1]), ms(t, 30));
        assert!(
            out.report.buttons.contains(X360Button::A),
            "без модификатора — базовое"
        );
        assert!(!out.report.buttons.contains(X360Button::B));
    }

    #[test]
    fn releasing_modifier_first_gives_no_phantom_base_press() {
        let mut profile = chord_profile(ModifierMode::LayerOnly);
        profile.keyboard = vec![keyboard(
            TARGET,
            Some(0),
            TriggerType::Direct,
            key(0x52, PressMode::HoldWhilePressed),
        )];
        let mut state = MapperState::default();
        let t = Instant::now();

        state.update(&profile, &buttons(&[0]), t);
        let out = state.update(&profile, &buttons(&[0, 1]), ms(t, 10));
        assert_eq!(out.keys.len(), 1);

        // Модификатор отпущен раньше целевой кнопки.
        let out = state.update(&profile, &buttons(&[1]), ms(t, 20));
        assert!(
            !out.report.buttons.contains(X360Button::A),
            "нет фантомного A"
        );
        assert!(
            !out.report.buttons.contains(X360Button::B),
            "вход назначен на клавишу — Xbox B в этом слое не срабатывает"
        );
        assert_eq!(out.keys.len(), 1, "клавиша держится до отпускания кнопки");

        let out = state.update(&profile, &buttons(&[]), ms(t, 30));
        assert!(out.report.buttons.is_empty());
        assert!(out.keys.is_empty(), "ничего не залипло");
        let out = state.update(&profile, &buttons(&[]), ms(t, 500));
        assert!(out.report.buttons.is_empty() && out.keys.is_empty());
    }

    #[test]
    fn single_double_and_long_press_are_separated_by_timing() {
        let mut profile = ControllerProfile::default_for(String::new());
        profile.entries.clear();
        let (single, double, long) = (0x46, 0x4D, 0x52);
        profile.keyboard = vec![
            keyboard(
                TARGET,
                None,
                TriggerType::SingleTap,
                key(single, PressMode::Pulse),
            ),
            keyboard(
                TARGET,
                None,
                TriggerType::DoubleTap,
                key(double, PressMode::Pulse),
            ),
            keyboard(
                TARGET,
                None,
                TriggerType::LongPress,
                key(long, PressMode::Pulse),
            ),
        ];
        let vks = |out: &MapperOutput| out.keys.iter().map(|key| key.vk).collect::<Vec<_>>();

        // Одиночное: ничего сразу, срабатывает после окна двойного нажатия.
        let mut state = MapperState::default();
        let t = Instant::now();
        assert!(vks(&state.update(&profile, &buttons(&[1]), t)).is_empty());
        assert!(vks(&state.update(&profile, &buttons(&[]), ms(t, 80))).is_empty());
        assert!(vks(&state.update(&profile, &buttons(&[]), ms(t, 200))).is_empty());
        assert_eq!(
            vks(&state.update(&profile, &buttons(&[]), ms(t, 340))),
            vec![single]
        );
        assert!(vks(&state.update(&profile, &buttons(&[]), ms(t, 400))).is_empty());

        // Двойное: второе нажатие внутри окна, одиночного нет.
        let mut state = MapperState::default();
        state.update(&profile, &buttons(&[1]), t);
        state.update(&profile, &buttons(&[]), ms(t, 60));
        assert_eq!(
            vks(&state.update(&profile, &buttons(&[1]), ms(t, 150))),
            vec![double]
        );
        state.update(&profile, &buttons(&[]), ms(t, 200));
        for at in [300, 500, 800] {
            assert!(vks(&state.update(&profile, &buttons(&[]), ms(t, at))).is_empty());
        }

        // Долгое: после порога удержания, при отпускании одиночного нет.
        let mut state = MapperState::default();
        state.update(&profile, &buttons(&[1]), t);
        assert!(vks(&state.update(&profile, &buttons(&[1]), ms(t, 300))).is_empty());
        assert_eq!(
            vks(&state.update(&profile, &buttons(&[1]), ms(t, 360))),
            vec![long]
        );
        state.update(&profile, &buttons(&[]), ms(t, 600));
        for at in [700, 1000] {
            assert!(vks(&state.update(&profile, &buttons(&[]), ms(t, at))).is_empty());
        }
    }

    #[test]
    fn direct_rule_has_zero_delay() {
        let mut profile = ControllerProfile::default_for(String::new());
        profile.keyboard = vec![keyboard(
            InputBinding::Button { index: 2 },
            None,
            TriggerType::Direct,
            key(0x46, PressMode::HoldWhilePressed),
        )];
        let mut state = MapperState::default();
        let t = Instant::now();
        let out = state.update(&profile, &buttons(&[2]), t);
        assert_eq!(out.keys.len(), 1);
        let out = state.update(&profile, &buttons(&[]), ms(t, 8));
        assert!(out.keys.is_empty());
    }

    #[test]
    fn tap_action_modifier_fires_only_without_combo() {
        let mut profile = chord_profile(ModifierMode::TapAction);
        profile
            .entries
            .push(MappingEntry::new(OutputControl::Y, MODIFIER));
        let mut state = MapperState::default();
        let t = Instant::now();

        // Нажали и отпустили модификатор один — короткий Y.
        let out = state.update(&profile, &buttons(&[0]), t);
        assert!(!out.report.buttons.contains(X360Button::Y));
        let out = state.update(&profile, &buttons(&[]), ms(t, 50));
        assert!(out.report.buttons.contains(X360Button::Y));
        let out = state.update(&profile, &buttons(&[]), ms(t, 200));
        assert!(!out.report.buttons.contains(X360Button::Y));

        // С комбинацией — Y не срабатывает.
        state.update(&profile, &buttons(&[0]), ms(t, 300));
        state.update(&profile, &buttons(&[0, 1]), ms(t, 310));
        state.update(&profile, &buttons(&[0]), ms(t, 320));
        let out = state.update(&profile, &buttons(&[]), ms(t, 330));
        assert!(!out.report.buttons.contains(X360Button::Y));
    }

    #[test]
    fn passthrough_modifier_keeps_its_own_button() {
        let mut profile = chord_profile(ModifierMode::Passthrough);
        profile
            .entries
            .push(MappingEntry::new(OutputControl::Y, MODIFIER));
        let mut state = MapperState::default();
        let t = Instant::now();
        let out = state.update(&profile, &buttons(&[0]), t);
        assert!(out.report.buttons.contains(X360Button::Y));
        let out = state.update(&profile, &buttons(&[0, 1]), ms(t, 10));
        assert!(out.report.buttons.contains(X360Button::B));
        assert!(!out.report.buttons.contains(X360Button::A));
    }

    #[test]
    fn old_profile_json_still_loads() {
        let json = r#"{"name":"old","device_guid":"g","entries":[
            {"control":"a","binding":{"kind":"button","index":0}}]}"#;
        let profile: ControllerProfile = serde_json::from_str(json).unwrap();
        assert_eq!(profile.entries[0].trigger, TriggerType::Direct);
        assert!(profile.modifiers.is_empty() && profile.keyboard.is_empty());
        assert_eq!(profile.timings.double_tap_window_ms, 250);
        assert_eq!(profile.timings.hold_threshold_ms, 350);
    }
}
