//! Pointer and wheel input to an offscreen browser.
//!
//! The host gives positions in surface pixels; CEF takes whole DIPs. The
//! last position, the held buttons, the modifiers and the click count are
//! state Kurogane keeps between calls.

use std::time::{Duration, Instant};

use tanso::MouseButtonType;

use crate::keys::{Modifiers, flag};

/// A mouse button. CEF sends no other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PointerButton {
    Left,
    Middle,
    Right,
}

impl PointerButton {
    /// CEF's flag of the button held.
    fn held(self) -> u32 {
        match self {
            Self::Left => flag::LEFT_MOUSE_BUTTON,
            Self::Middle => flag::MIDDLE_MOUSE_BUTTON,
            Self::Right => flag::RIGHT_MOUSE_BUTTON,
        }
    }

    /// CEF's button type.
    pub(crate) fn cef(self) -> MouseButtonType {
        match self {
            Self::Left => MouseButtonType::LEFT,
            Self::Middle => MouseButtonType::MIDDLE,
            Self::Right => MouseButtonType::RIGHT,
        }
    }
}

/// How far a wheel turned. Positive `y` reveals the content above, positive
/// `x` the content to the left.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum WheelDelta {
    /// Notches of a mouse wheel, each scrolling as far as the OS scrolls a
    /// windowed browser.
    Lines { x: f64, y: f64 },
    /// Pixels of a touchpad or a smooth wheel, scrolled exactly.
    Pixels { x: f64, y: f64 },
}

/// Pointer event as CEF takes it, in whole DIPs of the view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PointerEvent {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) modifiers: u32,
}

/// Bounds within which a press counts as one click more.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DoubleClick {
    /// Longest time from the last press.
    time: Duration,
    /// Width of the box around the last press in surface pixels.
    width: f64,
    /// Height of the box around the last press in surface pixels.
    height: f64,
}

impl DoubleClick {
    /// Chromium's bounds where the OS has none (`ui/events/event.cc`).
    const CHROMIUM: Self = Self {
        time: Duration::from_millis(500),
        width: 4.0,
        height: 4.0,
    };

    /// Returns the OS's bounds, Chromium's where the OS has none.
    pub(crate) fn of_os() -> Self {
        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                GetSystemMetrics, SM_CXDOUBLECLK, SM_CYDOUBLECLK,
            };
            // SAFETY: neither call takes arguments that point anywhere
            let (time, width, height) = unsafe {
                (
                    GetDoubleClickTime(),
                    GetSystemMetrics(SM_CXDOUBLECLK),
                    GetSystemMetrics(SM_CYDOUBLECLK),
                )
            };
            // Each call answers 0 when it fails
            let or_chromium =
                |value: f64, chromium: f64| if value > 0.0 { value } else { chromium };
            Self {
                time: if time > 0 {
                    Duration::from_millis(u64::from(time))
                } else {
                    Self::CHROMIUM.time
                },
                width: or_chromium(f64::from(width), Self::CHROMIUM.width),
                height: or_chromium(f64::from(height), Self::CHROMIUM.height),
            }
        }
        #[cfg(target_os = "macos")]
        {
            let interval = objc2_app_kit::NSEvent::doubleClickInterval();
            Self {
                time: Duration::try_from_secs_f64(interval).unwrap_or(Self::CHROMIUM.time),
                ..Self::CHROMIUM
            }
        }
        #[cfg(target_os = "linux")]
        {
            Self::CHROMIUM
        }
    }
}

/// Last press, for counting clicks.
#[derive(Clone, Copy, Debug)]
struct Press {
    at: Instant,
    position: (f64, f64),
    button: PointerButton,
    count: i32,
}

/// Pointer state of an offscreen browser.
#[derive(Debug)]
pub(crate) struct Pointer {
    /// Last position in surface pixels.
    position: (f64, f64),
    /// Held buttons as CEF's flags.
    buttons: u32,
    /// Modifiers as CEF's flags.
    modifiers: u32,
    double_click: DoubleClick,
    last_press: Option<Press>,
    /// Wheel pixels not scrolled yet, each under one.
    wheel_rest: (f64, f64),
}

impl Pointer {
    /// Pointer at the view's origin with nothing held.
    pub(crate) fn new(double_click: DoubleClick) -> Self {
        Self {
            position: (0.0, 0.0),
            buttons: 0,
            modifiers: 0,
            double_click,
            last_press: None,
            wheel_rest: (0.0, 0.0),
        }
    }

    /// Modifiers as CEF's flags.
    pub(crate) fn modifiers(&self) -> u32 {
        self.modifiers
    }

    /// Sets the modifiers every later event carries.
    pub(crate) fn set_modifiers(&mut self, modifiers: Modifiers) {
        self.modifiers = modifiers.state();
    }

    /// Returns the event at the last position for a view at `scale`.
    pub(crate) fn event(&self, scale: f64) -> PointerEvent {
        let dip = |pixels: f64| (pixels / scale).floor() as i32;
        PointerEvent {
            x: dip(self.position.0),
            y: dip(self.position.1),
            modifiers: self.modifiers | self.buttons,
        }
    }

    /// Moves to `(x, y)` and returns the event there.
    pub(crate) fn moved(&mut self, x: f64, y: f64, scale: f64) -> PointerEvent {
        self.position = (x, y);
        self.event(scale)
    }

    /// Presses `button` at `(x, y)` at `at` and returns the event with its
    /// click count, at most three as Chromium counts.
    pub(crate) fn pressed(
        &mut self,
        x: f64,
        y: f64,
        button: PointerButton,
        scale: f64,
        at: Instant,
    ) -> (PointerEvent, i32) {
        self.position = (x, y);
        self.buttons |= button.held();
        let bounds = self.double_click;
        let repeated = self.last_press.is_some_and(|last| {
            last.button == button
                && at.saturating_duration_since(last.at) <= bounds.time
                && (x - last.position.0).abs() <= bounds.width / 2.0
                && (y - last.position.1).abs() <= bounds.height / 2.0
        });
        let count = match self.last_press {
            Some(last) if repeated => (last.count + 1).min(3),
            _ => 1,
        };
        self.last_press = Some(Press {
            at,
            position: (x, y),
            button,
            count,
        });
        (self.event(scale), count)
    }

    /// Releases `button` at `(x, y)` and returns the event with its press's
    /// click count.
    pub(crate) fn released(
        &mut self,
        x: f64,
        y: f64,
        button: PointerButton,
        scale: f64,
    ) -> (PointerEvent, i32) {
        self.position = (x, y);
        self.buttons &= !button.held();
        let count = self
            .last_press
            .filter(|last| last.button == button)
            .map_or(1, |last| last.count);
        (self.event(scale), count)
    }

    /// Turns the wheel by `delta` at the last position. Returns the event and
    /// CEF's whole units, None while they come to nothing.
    pub(crate) fn wheel(
        &mut self,
        delta: WheelDelta,
        scale: f64,
    ) -> Option<(PointerEvent, (i32, i32))> {
        let pixels = match delta {
            WheelDelta::Lines { x, y } => (x * notch_pixels(true), y * notch_pixels(false)),
            WheelDelta::Pixels { x, y } => (x, y),
        };
        let total = (self.wheel_rest.0 + pixels.0, self.wheel_rest.1 + pixels.1);
        let ((across, scrolled_x), (down, scrolled_y)) = (
            pixel_units(total.0.trunc(), true),
            pixel_units(total.1.trunc(), false),
        );
        // What CEF scrolls short or past stays for the next turn
        self.wheel_rest = (
            total.0 - f64::from(scrolled_x),
            total.1 - f64::from(scrolled_y),
        );
        let units = (across, down);
        (units != (0, 0)).then(|| (self.event(scale), units))
    }
}

/// Returns the pixels a notch of a mouse wheel scrolls a windowed browser
/// `across` or down. Windows scrolls 100/3 pixels a line or character of its
/// settings.
#[cfg(target_os = "windows")]
fn notch_pixels(across: bool) -> f64 {
    f64::from(scroll_count(across)) * 100.0 / 3.0
}

/// Returns the pixels a notch of a mouse wheel scrolls a windowed browser,
/// Chromium's 120 on Linux.
#[cfg(target_os = "linux")]
fn notch_pixels(_across: bool) -> f64 {
    120.0
}

/// Returns the pixels a notch of a mouse wheel scrolls a windowed browser,
/// Chromium's 40 a tick on macOS.
#[cfg(target_os = "macos")]
fn notch_pixels(_across: bool) -> f64 {
    40.0
}

/// Returns Windows' characters a notch `across` or lines a notch down.
#[cfg(target_os = "windows")]
fn scroll_count(across: bool) -> f32 {
    use std::ffi::c_void;

    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SPI_GETWHEELSCROLLCHARS, SPI_GETWHEELSCROLLLINES, SystemParametersInfoW,
    };
    let action = if across {
        SPI_GETWHEELSCROLLCHARS
    } else {
        SPI_GETWHEELSCROLLLINES
    };
    let mut count: u32 = 3;
    // SAFETY: both actions write one u32 through the pointer
    let read = unsafe { SystemParametersInfoW(action, 0, (&raw mut count).cast::<c_void>(), 0) };
    // No scrolling and a page a notch scale nothing CEF can invert
    if read == 0 || count == 0 || count == u32::MAX {
        count = 3;
    }
    count as f32
}

/// Returns CEF's whole wheel units of `pixels` whole pixels, `across` or down,
/// with the pixels CEF scrolls for them.
#[cfg(target_os = "windows")]
fn pixel_units(pixels: f64, across: bool) -> (i32, i32) {
    let count = scroll_count(across);
    let units = units_for_pixels(pixels, count);
    (units, cef_pixels(units, count))
}

/// Returns CEF's whole wheel units of `pixels` whole pixels with the pixels
/// CEF scrolls for them, which macOS and Linux take as pixels.
#[cfg(not(target_os = "windows"))]
fn pixel_units(pixels: f64, _across: bool) -> (i32, i32) {
    (pixels as i32, pixels as i32)
}

/// Returns CEF's pixels of a wheel event of `units` at `count` lines or
/// characters a notch. Windows scales in f32 at 100/3 pixels each and
/// truncates (`browser_platform_delegate_native_win.cc`).
#[cfg(target_os = "windows")]
fn cef_pixels(units: i32, count: f32) -> i32 {
    (units as f32 / 120.0 * (count * (100.0_f32 / 3.0))) as i32
}

/// Returns the fewest whole units CEF makes at least `pixels` whole pixels of
/// at `count` lines or characters a notch. Up to three a notch each pixel has
/// its units; past that a unit is more than a pixel.
#[cfg(target_os = "windows")]
fn units_for_pixels(pixels: f64, count: f32) -> i32 {
    let target = pixels.abs() as i32;
    let mut units = (f64::from(target) * 120.0 / (f64::from(count) * 100.0 / 3.0)).floor() as i32;
    while cef_pixels(units, count) < target {
        units += 1;
    }
    if pixels < 0.0 { -units } else { units }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: DoubleClick = DoubleClick::CHROMIUM;

    #[test]
    fn positions_floor_to_whole_dips() {
        let mut pointer = Pointer::new(BOUNDS);
        assert_eq!(
            pointer.moved(101.0, 51.0, 2.0),
            PointerEvent {
                x: 50,
                y: 25,
                modifiers: 0
            }
        );
        assert_eq!(pointer.moved(149.9, 0.0, 1.5).x, 99);
        assert_eq!(pointer.moved(-1.0, 0.0, 1.0).x, -1);
    }

    #[test]
    fn events_carry_the_modifiers_and_the_held_buttons() {
        let mut pointer = Pointer::new(BOUNDS);
        pointer.set_modifiers(Modifiers::SHIFT | Modifiers::CONTROL);
        let (down, _) = pointer.pressed(0.0, 0.0, PointerButton::Left, 1.0, Instant::now());
        assert_eq!(
            down.modifiers,
            flag::SHIFT_DOWN | flag::CONTROL_DOWN | flag::LEFT_MOUSE_BUTTON
        );
        assert_eq!(
            pointer.moved(10.0, 0.0, 1.0).modifiers & flag::LEFT_MOUSE_BUTTON,
            flag::LEFT_MOUSE_BUTTON
        );
        let (up, _) = pointer.released(10.0, 0.0, PointerButton::Left, 1.0);
        assert_eq!(up.modifiers, flag::SHIFT_DOWN | flag::CONTROL_DOWN);
    }

    #[test]
    fn quick_presses_in_place_count_up_to_three() {
        let mut pointer = Pointer::new(BOUNDS);
        let start = Instant::now();
        let mut count = |x: f64, button, after: u64| {
            pointer
                .pressed(x, 0.0, button, 1.0, start + Duration::from_millis(after))
                .1
        };
        assert_eq!(count(0.0, PointerButton::Left, 0), 1);
        assert_eq!(count(1.0, PointerButton::Left, 100), 2);
        assert_eq!(count(1.0, PointerButton::Left, 200), 3);
        assert_eq!(count(1.0, PointerButton::Left, 300), 3);
        // Too far, too late and another button start over
        assert_eq!(count(10.0, PointerButton::Left, 400), 1);
        assert_eq!(count(10.0, PointerButton::Left, 1000), 1);
        assert_eq!(count(10.0, PointerButton::Right, 1100), 1);
    }

    #[test]
    fn a_release_has_its_press_s_count() {
        let mut pointer = Pointer::new(BOUNDS);
        let start = Instant::now();
        pointer.pressed(0.0, 0.0, PointerButton::Left, 1.0, start);
        pointer.pressed(
            0.0,
            0.0,
            PointerButton::Left,
            1.0,
            start + Duration::from_millis(50),
        );
        assert_eq!(pointer.released(0.0, 0.0, PointerButton::Left, 1.0).1, 2);
        assert_eq!(pointer.released(0.0, 0.0, PointerButton::Middle, 1.0).1, 1);
    }

    #[test]
    fn lines_scroll_the_way_they_turn() {
        let mut pointer = Pointer::new(BOUNDS);
        let mut units = |y: f64| {
            pointer
                .wheel(WheelDelta::Lines { x: 0.0, y }, 1.0)
                .map_or((0, 0), |(_, units)| units)
        };
        assert!(units(1.0).1 > 0);
        assert!(units(-2.0).1 < 0);
    }

    #[test]
    fn half_pixels_send_every_second_time() {
        let mut pointer = Pointer::new(BOUNDS);
        let sent = (0..10)
            .filter(|_| {
                pointer
                    .wheel(WheelDelta::Pixels { x: 0.0, y: -0.5 }, 1.0)
                    .is_some()
            })
            .count();
        assert_eq!(sent, 5);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_units_land_on_whole_pixels() {
        for count in [1.0, 2.0, 3.0, 5.0] {
            for pixels in [1.0_f64, 5.0, 30.0, 119.0, 120.0, -7.0] {
                let units = units_for_pixels(pixels, count);
                let scrolled = cef_pixels(units, count).abs();
                let fewer = cef_pixels(units - units.signum(), count).abs();
                let target = pixels.abs() as i32;
                assert!(scrolled >= target && fewer < target, "{pixels} at {count}");
                if count <= 3.0 {
                    assert_eq!(scrolled, target, "{pixels} at {count}");
                }
            }
        }
        assert_eq!(units_for_pixels(0.0, 2.0), 0);
    }
}
