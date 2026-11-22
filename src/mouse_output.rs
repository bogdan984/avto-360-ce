//! Движение мыши через Win32 SendInput (относительное смещение — для
//! обзора камеры в играх). Скорость задаётся в пикселях в секунду, дробные
//! остатки накапливаются между опросами, чтобы малые скорости тоже работали.

use std::{
    mem::size_of,
    time::{Duration, Instant},
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT,
};

/// Больше этого шага за один опрос не берём (после паузы мышь не прыгает).
const MAX_STEP: Duration = Duration::from_millis(50);

#[derive(Default)]
pub struct MouseOutput {
    remainder: (f32, f32),
    last: Option<Instant>,
}

impl MouseOutput {
    /// Сдвинуть мышь на столько, сколько прошла бы со скоростью `velocity`.
    pub fn update(&mut self, velocity: (f32, f32), now: Instant) {
        let (dx, dy) = self.step(velocity, now);
        if dx != 0 || dy != 0 {
            send_move(dx, dy);
        }
    }

    /// Смещение в пикселях за время с прошлого вызова.
    fn step(&mut self, velocity: (f32, f32), now: Instant) -> (i32, i32) {
        if velocity == (0.0, 0.0) {
            *self = Self::default();
            return (0, 0);
        }
        let dt = self
            .last
            .map(|last| now.saturating_duration_since(last).min(MAX_STEP))
            .unwrap_or_default()
            .as_secs_f32();
        self.last = Some(now);
        self.remainder.0 += velocity.0 * dt;
        self.remainder.1 += velocity.1 * dt;
        let dx = self.remainder.0.trunc();
        let dy = self.remainder.1.trunc();
        self.remainder.0 -= dx;
        self.remainder.1 -= dy;
        (dx as i32, dy as i32)
    }
}

fn send_move(dx: i32, dy: i32) {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        SendInput(1, &input, size_of::<INPUT>() as i32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_by_speed_times_time() {
        let mut mouse = MouseOutput::default();
        let start = Instant::now();
        assert_eq!(mouse.step((1000.0, -500.0), start), (0, 0), "первый опрос — точка отсчёта");
        assert_eq!(mouse.step((1000.0, -500.0), start + Duration::from_millis(10)), (10, -5));
    }

    #[test]
    fn slow_speed_accumulates_fractions() {
        let mut mouse = MouseOutput::default();
        let start = Instant::now();
        mouse.step((20.0, 0.0), start);
        let mut total = 0;
        for tick in 1..=125 {
            total += mouse.step((20.0, 0.0), start + Duration::from_millis(8 * tick)).0;
        }
        // 20 px/с × 1 с = 20 пикселей, остаток не теряется.
        assert!((19..=20).contains(&total), "total = {total}");
    }

    #[test]
    fn stop_resets_and_long_pause_is_capped() {
        let mut mouse = MouseOutput::default();
        let start = Instant::now();
        mouse.step((1000.0, 0.0), start);
        assert_eq!(mouse.step((0.0, 0.0), start + Duration::from_millis(10)), (0, 0));
        mouse.step((1000.0, 0.0), start + Duration::from_secs(1));
        assert_eq!(
            mouse.step((1000.0, 0.0), start + Duration::from_secs(3)),
            (50, 0),
            "пауза ограничена 50 мс"
        );
    }
}
