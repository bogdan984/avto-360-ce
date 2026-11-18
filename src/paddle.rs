//! Разделение педали и лепестка на одной оси. Некоторые рули отдают
//! лепесток на ту же ось, что и педаль: педаль двигает ось плавно, лепесток —
//! скачком до упора. Скачок за короткое окно считаем лепестком: он становится
//! отдельным входом, а ось на это время держит последнее значение педали.

use crate::model::{PaddleAxis, RawState};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// Ось считается «у упора» от этой доли хода.
const EXTREME: f32 = 0.97;
/// Лепесток отпущен, когда ось ушла от упора ниже этой доли.
const RELEASE: f32 = 0.94;
const HISTORY: Duration = Duration::from_millis(150);

struct AxisState {
    axis: u32,
    history: VecDeque<(Instant, i16)>,
    /// Зажатый лепесток: Some(отрицательное направление).
    active: Option<bool>,
    /// Значение педали до скачка.
    held: i16,
}

#[derive(Default)]
pub struct PaddleFilter {
    axes: Vec<AxisState>,
}

impl PaddleFilter {
    pub fn reset(&mut self) {
        self.axes.clear();
    }

    /// Подменяет оси с лепестками значением педали и дописывает зажатые лепестки
    /// в `raw.paddles`.
    pub fn apply(&mut self, config: &[PaddleAxis], raw: &mut RawState, now: Instant) {
        self.axes
            .retain(|state| config.iter().any(|item| item.axis == state.axis));
        for item in config {
            let index = item.axis as usize;
            let Some(value) = raw.axes.get(index).copied() else {
                continue;
            };
            let position = match self.axes.iter().position(|state| state.axis == item.axis) {
                Some(position) => position,
                None => {
                    self.axes.push(AxisState {
                        axis: item.axis,
                        history: VecDeque::new(),
                        active: None,
                        held: 0,
                    });
                    self.axes.len() - 1
                }
            };
            let state = &mut self.axes[position];

            if let Some(negative) = state.active {
                if magnitude(value, negative) < RELEASE {
                    state.active = None;
                }
            } else {
                state.active = detect_jump(state, item, value, now);
            }
            if let Some(negative) = state.active {
                raw.axes[index] = state.held;
                raw.paddles.push((item.axis, negative));
            }

            state.history.push_back((now, value));
            while state
                .history
                .front()
                .is_some_and(|(at, _)| now.duration_since(*at) > HISTORY)
            {
                state.history.pop_front();
            }
        }
    }
}

/// Ось только что дошла до упора, а в пределах окна была ниже на `min_jump`.
fn detect_jump(state: &mut AxisState, item: &PaddleAxis, value: i16, now: Instant) -> Option<bool> {
    let window = Duration::from_millis(u64::from(item.window_ms));
    for negative in [true, false] {
        if magnitude(value, negative) < EXTREME {
            continue;
        }
        let was_below = state
            .history
            .back()
            .is_some_and(|(_, previous)| magnitude(*previous, negative) < EXTREME);
        let start = state.history.iter().find(|(at, previous)| {
            now.duration_since(*at) <= window
                && magnitude(*previous, negative) <= EXTREME - item.min_jump
        });
        if let (true, Some((_, before))) = (was_below, start) {
            state.held = *before;
            return Some(negative);
        }
    }
    None
}

/// Отклонение оси в направлении (0..1).
fn magnitude(value: i16, negative: bool) -> f32 {
    let normalized = if value < 0 {
        value as f32 / 32_768.0
    } else {
        value as f32 / 32_767.0
    };
    if negative {
        (-normalized).max(0.0)
    } else {
        normalized.max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AXIS: u32 = 2;

    fn raw(z: i16) -> RawState {
        RawState {
            axes: vec![0, 0, z],
            ..Default::default()
        }
    }

    /// Прогоняет значения оси с шагом 8 мс, возвращает (ось, лепестки) на каждом шаге.
    fn run(values: &[i16]) -> Vec<(i16, Vec<(u32, bool)>)> {
        let config = [PaddleAxis::new(AXIS)];
        let mut filter = PaddleFilter::default();
        let start = Instant::now();
        values
            .iter()
            .enumerate()
            .map(|(step, value)| {
                let mut state = raw(*value);
                filter.apply(
                    &config,
                    &mut state,
                    start + Duration::from_millis(step as u64 * 8),
                );
                (state.axes[2], state.paddles)
            })
            .collect()
    }

    #[test]
    fn paddle_jump_is_separated_and_pedal_value_is_held() {
        // Газ на 30% (ось влево), затем лепесток — скачок до упора, затем отпускание.
        let out = run(&[-9_830, -9_830, -32_768, -32_768, -9_830, -9_830]);
        assert!(out[1].1.is_empty());
        assert_eq!(
            out[2],
            (-9_830, vec![(AXIS, true)]),
            "лепесток, педаль держится"
        );
        assert_eq!(out[3], (-9_830, vec![(AXIS, true)]));
        assert_eq!(out[4], (-9_830, vec![]), "отпустили — снова педаль");
    }

    #[test]
    fn slow_pedal_to_full_stays_pedal() {
        // Педаль плавно до упора: примерно 10% за 8 мс.
        let values: Vec<i16> = (0..=11)
            .map(|step| -(step * 3_000).min(32_768) as i16)
            .collect();
        let out = run(&values);
        assert!(out.iter().all(|(_, paddles)| paddles.is_empty()));
        assert_eq!(out.last().unwrap().0, values[11], "педаль доходит до упора");
    }

    #[test]
    fn opposite_paddle_while_braking_keeps_brake() {
        // Тормоз на 50% (вправо), затем лепесток газа (скачок влево до упора).
        let out = run(&[16_384, 16_384, -32_768, 16_384]);
        assert_eq!(out[2], (16_384, vec![(AXIS, true)]));
        assert_eq!(out[3], (16_384, vec![]));
    }
}
