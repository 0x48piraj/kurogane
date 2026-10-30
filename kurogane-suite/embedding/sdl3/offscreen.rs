//! Chromium offscreen, its frames drawn by SDL3 in a window SDL3 makes.
//!
//! The browser has no window. Each frame comes to a callback inside
//! Kurogane's pump on the main thread. The callback keeps what changed in a
//! buffer the loop shares through an `Rc`. The loop draws that buffer through
//! a streaming texture. The window's size in pixels and its display scale go
//! to the browser as they change. The page's cursor comes as an event.
//!
//! The mouse, the wheel, the keys and the IME go to the browser. SDL's mouse
//! moves in window points, which the browser takes in pixels. A key's text
//! comes from SDL as the event after its key-down. The focused field starts
//! and stops SDL's text input and places its IME.
//!
//! Windows and macOS run a loop of their own while the user moves or resizes
//! the window. SDL's loop waits until it ends. An event watch keeps the
//! browser's size and place and pumps Chromium there.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use kurogane::{
    AppInstance, InputPurpose, KeyInput, LogicalKey, Modifiers, MouseCursor, OffscreenBrowser,
    OffscreenEvent, OffscreenOptions, PhysicalCode, PixelFrame, PixelRect, PointerButton,
    WheelDelta,
};
use kurogane_suite::embedding::{self, PumpSchedule};
use sdl3::VideoSubsystem;
use sdl3::event::{Event, EventSender, WindowEvent};
use sdl3::keyboard::{Keycode, Mod, Scancode, TextInputOptions, TextInputType};
use sdl3::mouse::{Cursor, MouseButton, SystemCursor};
use sdl3::pixels::PixelFormat;
use sdl3::rect::Rect;
use sdl3::render::{FRect, Texture, TextureCreator};
use sdl3::video::{Window, WindowContext};

/// User event that wakes SDL's loop.
struct Wake;

/// What SDL's loop and the event watch share on the main thread.
struct Host {
    runtime: AppInstance,
    schedule: PumpSchedule,
    browser: OffscreenBrowser,
    /// The window's pixels a point, as the loop last saw them.
    density: Cell<f64>,
}

thread_local! {
    /// The host once SDL's loop runs. SDL also calls the event watch on each
    /// thread that pushes an event, where this stays empty.
    static HOST: RefCell<Option<Rc<Host>>> = const { RefCell::new(None) };
}

impl Host {
    /// Answers each event as SDL sends it, inside the system's move and
    /// resize loop too.
    fn watch(&self, event: &Event) {
        let Event::Window { win_event, .. } = event else {
            return;
        };
        match *win_event {
            WindowEvent::Exposed => self.schedule.pump_if_due(&self.runtime),
            WindowEvent::PixelSizeChanged(width, height) => self.browser.set_size(
                u32::try_from(width).unwrap_or(0),
                u32::try_from(height).unwrap_or(0),
            ),
            WindowEvent::Moved(x, y) => {
                let pixels = |points: i32| (f64::from(points) * self.density.get()).round() as i32;
                self.browser.set_screen_origin(pixels(x), pixels(y));
            }
            _ => {}
        }
    }
}

/// Latest frame of the page, as the callback leaves it for the loop.
#[derive(Default)]
struct Frame {
    bgra: Vec<u8>,
    width: u32,
    height: u32,
    /// Changed since the loop last drew it
    fresh: bool,
}

impl Frame {
    /// Keeps what changed in `frame`.
    fn keep(&mut self, frame: PixelFrame<'_>) {
        if (frame.width(), frame.height()) != (self.width, self.height) {
            self.bgra = frame.bytes().to_vec();
            self.width = frame.width();
            self.height = frame.height();
        } else {
            let stride = frame.stride();
            for rect in frame.damage() {
                for row in rect.y..(rect.y + rect.height).min(frame.height()) {
                    let start = row as usize * stride + rect.x as usize * 4;
                    let end = start + rect.width.min(frame.width() - rect.x) as usize * 4;
                    self.bgra[start..end].copy_from_slice(&frame.bytes()[start..end]);
                }
            }
        }
        self.fresh = true;
    }
}

fn main() {
    kurogane_suite::logging();

    // Kurogane starts before SDL. On macOS it installs the NSApplication
    // subclass CEF needs; the wake reaches SDL's loop once SDL runs
    let sender: Arc<OnceLock<EventSender>> = Arc::default();
    let wake = {
        let sender = Arc::clone(&sender);
        move || {
            if let Some(sender) = sender.get() {
                let _ = sender.push_custom_event(Wake);
            }
        }
    };
    let (runtime, schedule) = embedding::start_app(embedding::app().offscreen(), wake.clone())
        .expect("Kurogane failed to start");

    let sdl = sdl3::init().expect("SDL failed to start");
    let video = sdl.video().expect("SDL has no video");
    let events = sdl.event().expect("SDL has no events");
    events
        .register_custom_event::<Wake>()
        .expect("SDL registers the wake event");
    let _ = sender.set(events.event_sender());

    let window = video
        .window("Kurogane in SDL3 offscreen", 1000, 700)
        .resizable()
        .high_pixel_density()
        .build()
        .expect("SDL made no window");
    let mut canvas = window.into_canvas();
    let creator = canvas.texture_creator();

    let frame = Rc::new(RefCell::new(Frame::default()));
    let (width, height) = canvas.window().size_in_pixels();
    let browser = runtime
        .create_offscreen_browser(
            &embedding::page("SDL3 offscreen"),
            OffscreenOptions::new(width, height, scale(canvas.window()))
                .on_pixels({
                    let frame = Rc::clone(&frame);
                    move |pixels| frame.borrow_mut().keep(pixels)
                })
                .wake(wake),
        )
        .expect("the offscreen browser could not be created");
    place(&browser, canvas.window());
    let host = Rc::new(Host {
        runtime,
        schedule,
        browser,
        density: Cell::new(f64::from(canvas.window().pixel_density())),
    });
    HOST.with_borrow_mut(|shared| *shared = Some(Rc::clone(&host)));
    let watch = events.add_event_watch(|event: Event| {
        let host = HOST.with_borrow(Option::clone);
        if let Some(host) = host {
            host.watch(&event);
        }
    });
    let Host {
        runtime,
        schedule,
        browser,
        ..
    } = &*host;

    let mut texture: Option<Texture<'_>> = None;
    // Kept while it shows; SDL shows a cursor only while its handle lives
    let mut cursor: Option<Cursor>;
    let mut pump = sdl.event_pump().expect("SDL has an event pump");
    while !runtime.should_shutdown() {
        let first = pump.wait_event_timeout_ms(schedule.wait_ms());
        // A key-down waits for the text SDL sends after it
        let mut held: Option<KeyInput> = None;
        for event in first.into_iter().chain(pump.poll_iter()) {
            if let Event::TextInput { text, .. } = &event {
                match held.take() {
                    Some(key) => browser.key_down(&key.text(text.as_str())),
                    None => browser.ime_commit(text),
                }
                continue;
            }
            if let Some(key) = held.take() {
                browser.key_down(&key);
            }
            let density = f64::from(canvas.window().pixel_density());
            host.density.set(density);
            match event {
                Event::Window {
                    win_event: WindowEvent::CloseRequested,
                    ..
                }
                | Event::Quit { .. } => runtime.handle().close_all_browsers(true),
                // The watch takes the size and the place; a move to a display
                // of another scale changes the scale too
                Event::Window {
                    win_event: WindowEvent::DisplayChanged(..),
                    ..
                } => {
                    browser.set_scale(scale(canvas.window()));
                    place(browser, canvas.window());
                }
                Event::Window {
                    win_event: WindowEvent::FocusGained,
                    ..
                } => browser.set_focused(true),
                Event::Window {
                    win_event: WindowEvent::FocusLost,
                    ..
                } => browser.set_focused(false),
                Event::Window {
                    win_event: WindowEvent::MouseLeave,
                    ..
                } => browser.pointer_leave(),
                Event::MouseMotion { x, y, .. } => {
                    browser.pointer_move(f64::from(x) * density, f64::from(y) * density);
                }
                Event::MouseButtonDown {
                    mouse_btn, x, y, ..
                } => {
                    if let Some(button) = pointer_button(mouse_btn) {
                        browser.pointer_down(
                            f64::from(x) * density,
                            f64::from(y) * density,
                            button,
                        );
                    }
                }
                Event::MouseButtonUp {
                    mouse_btn, x, y, ..
                } => {
                    if let Some(button) = pointer_button(mouse_btn) {
                        browser.pointer_up(f64::from(x) * density, f64::from(y) * density, button);
                    }
                }
                // SDL's x grows to the right, Kurogane's reveals the left; the
                // OS's natural scrolling is already in both
                Event::MouseWheel { x, y, .. } => browser.wheel(WheelDelta::Lines {
                    x: -f64::from(x),
                    y: f64::from(y),
                }),
                Event::KeyDown {
                    scancode,
                    keycode,
                    keymod,
                    repeat,
                    ..
                } => {
                    browser.set_modifiers(modifiers(keymod));
                    held = Some(key_input(scancode, keycode).repeat(repeat));
                }
                Event::KeyUp {
                    scancode,
                    keycode,
                    keymod,
                    ..
                } => {
                    browser.set_modifiers(modifiers(keymod));
                    browser.key_up(&key_input(scancode, keycode));
                }
                Event::TextEditing {
                    text,
                    start,
                    length,
                    ..
                } => browser.ime_preedit(&text, editing_cursor(&text, start, length)),
                _ => {}
            }
        }
        if let Some(key) = held.take() {
            browser.key_down(&key);
        }
        schedule.pump_if_due(runtime);

        for event in browser.events() {
            match event {
                OffscreenEvent::Cursor(shape) => {
                    cursor = Cursor::from_system(system_cursor(&shape)).ok();
                    if let Some(cursor) = &cursor {
                        cursor.set();
                    }
                }
                OffscreenEvent::Tooltip(Some(text)) => println!("tooltip: {text}"),
                OffscreenEvent::InputPurpose(purpose) => {
                    text_input(&video, canvas.window(), purpose);
                }
                OffscreenEvent::ImeArea(area) => {
                    let density = f64::from(canvas.window().pixel_density());
                    video
                        .text_input()
                        .set_rect(canvas.window(), window_rect(area, density), 0);
                }
                _ => {}
            }
        }

        let mut latest = frame.borrow_mut();
        if latest.fresh {
            latest.fresh = false;
            draw(&mut canvas, &creator, &mut texture, &latest);
        }
    }

    drop(texture);
    drop(watch);
    HOST.with_borrow_mut(|shared| *shared = None);
    let Some(Host {
        runtime, browser, ..
    }) = Rc::into_inner(host)
    else {
        unreachable!("the loop and the watch hold the host no more");
    };
    drop(browser);
    runtime.shutdown();
}

/// Returns the window's device scale factor, its pixel density times the
/// display's content scale.
fn scale(window: &Window) -> f64 {
    f64::from(window.display_scale())
}

/// Draws `frame` at its size in pixels from the window's top-left corner,
/// through `texture`, made again when the size changes.
fn draw<'a>(
    canvas: &mut sdl3::render::WindowCanvas,
    creator: &'a TextureCreator<WindowContext>,
    texture: &mut Option<Texture<'a>>,
    frame: &Frame,
) {
    let fits = texture
        .as_ref()
        .is_some_and(|texture| (texture.width(), texture.height()) == (frame.width, frame.height));
    if !fits {
        *texture = creator
            .create_texture_streaming(PixelFormat::BGRA32, frame.width, frame.height)
            .inspect_err(|e| eprintln!("SDL made no texture: {e}"))
            .ok();
    }
    let Some(texture) = texture else {
        return;
    };
    if let Err(e) = texture.update(None, &frame.bgra, frame.width as usize * 4) {
        eprintln!("SDL took no frame: {e}");
        return;
    }
    canvas.clear();
    let place = FRect::new(0.0, 0.0, frame.width as f32, frame.height as f32);
    let _ = canvas.copy(texture, None, place);
    canvas.present();
}

/// Tells `browser` where `window`'s surface sits on the screen, in pixels.
fn place(browser: &OffscreenBrowser, window: &Window) {
    let (x, y) = window.position();
    let density = f64::from(window.pixel_density());
    let pixels = |points: i32| (f64::from(points) * density).round() as i32;
    browser.set_screen_origin(pixels(x), pixels(y));
}

/// Returns the browser's button of SDL's `button`, None for one it lacks.
fn pointer_button(button: MouseButton) -> Option<PointerButton> {
    match button {
        MouseButton::Left => Some(PointerButton::Left),
        MouseButton::Middle => Some(PointerButton::Middle),
        MouseButton::Right => Some(PointerButton::Right),
        _ => None,
    }
}

/// Returns the modifier keys held and the locks on in SDL's `keymod`.
fn modifiers(keymod: Mod) -> Modifiers {
    let pairs = [
        (Mod::LSHIFTMOD | Mod::RSHIFTMOD, Modifiers::SHIFT),
        (Mod::LCTRLMOD | Mod::RCTRLMOD, Modifiers::CONTROL),
        (Mod::LALTMOD | Mod::RALTMOD, Modifiers::ALT),
        (Mod::LGUIMOD | Mod::RGUIMOD, Modifiers::META),
        (Mod::CAPSMOD, Modifiers::CAPS_LOCK),
        (Mod::NUMMOD, Modifiers::NUM_LOCK),
    ];
    pairs
        .into_iter()
        .filter(|(sdl, _)| keymod.intersects(*sdl))
        .fold(Modifiers::NONE, |held, (_, modifier)| held | modifier)
}

/// Returns the key of SDL's `scancode` and `keycode`. A scancode is a USB
/// HID usage of the keyboard page.
fn key_input(scancode: Option<Scancode>, keycode: Option<Keycode>) -> KeyInput {
    let code = scancode
        .and_then(|scancode| u32::try_from(scancode.to_i32()).ok())
        .and_then(|usage| PhysicalCode::from_hid_usage(0x07_0000 | usage))
        .unwrap_or(PhysicalCode::Unidentified);
    let key = keycode.map_or(LogicalKey::Unidentified, |keycode| {
        logical_key(keycode, code)
    });
    let input = KeyInput::new(code, key);
    // SDL types nothing for Enter, whose character forms and buttons act on
    if matches!(code, PhysicalCode::Enter | PhysicalCode::NumpadEnter) {
        input.text("\r")
    } else {
        input
    }
}

/// Returns what an SDL key means: the character of a printable keycode, a
/// named key by its place for any other.
fn logical_key(keycode: Keycode, code: PhysicalCode) -> LogicalKey {
    if let Some(c) = char::from_u32(keycode.to_ll().0).filter(|c| !c.is_control()) {
        return LogicalKey::Character(c.to_string());
    }
    match code {
        PhysicalCode::ShiftLeft | PhysicalCode::ShiftRight => LogicalKey::Shift,
        PhysicalCode::ControlLeft | PhysicalCode::ControlRight => LogicalKey::Control,
        PhysicalCode::AltLeft | PhysicalCode::AltRight => LogicalKey::Alt,
        PhysicalCode::MetaLeft | PhysicalCode::MetaRight => LogicalKey::Meta,
        PhysicalCode::NumpadEnter => LogicalKey::Enter,
        // Arrows, function keys, Enter, Escape and the like share their
        // place's name
        code => match LogicalKey::from_w3c(code.as_str()) {
            LogicalKey::Character(_) => LogicalKey::Unidentified,
            named => named,
        },
    }
}

/// Returns the byte range of SDL's editing cursor, `start` and `length` in
/// characters of `text`; None when SDL gives none.
fn editing_cursor(text: &str, start: i32, length: i32) -> Option<(usize, usize)> {
    let byte = |chars: usize| {
        text.char_indices()
            .nth(chars)
            .map_or(text.len(), |(at, _)| at)
    };
    let start = usize::try_from(start).ok()?;
    let end = start + usize::try_from(length).unwrap_or(0);
    Some((byte(start), byte(end)))
}

/// Starts SDL's text input for a field of `purpose`, stops it for none.
fn text_input(video: &VideoSubsystem, window: &Window, purpose: InputPurpose) {
    let input_type = match purpose {
        InputPurpose::None => {
            video.text_input().stop(window);
            return;
        }
        InputPurpose::Password => TextInputType::PasswordHidden,
        InputPurpose::Email => TextInputType::Email,
        InputPurpose::Number => TextInputType::Number,
        _ => TextInputType::Text,
    };
    let options = TextInputOptions {
        input_type: Some(input_type),
        ..Default::default()
    };
    if let Err(e) = video.text_input().start_with_options(window, options) {
        eprintln!("SDL started no text input: {e}");
    }
}

/// Returns `area` of surface pixels in the window's points.
fn window_rect(area: PixelRect, density: f64) -> Rect {
    let points = |pixels: u32| (f64::from(pixels) / density).round() as i32;
    Rect::new(
        points(area.x),
        points(area.y),
        points(area.width).max(1) as u32,
        points(area.height).max(1) as u32,
    )
}

/// Returns SDL's nearest cursor to the page's.
fn system_cursor(cursor: &MouseCursor) -> SystemCursor {
    match cursor {
        MouseCursor::Pointer | MouseCursor::Grab | MouseCursor::Grabbing => SystemCursor::Hand,
        MouseCursor::Text | MouseCursor::VerticalText => SystemCursor::IBeam,
        MouseCursor::Wait => SystemCursor::Wait,
        MouseCursor::Progress => SystemCursor::WaitArrow,
        MouseCursor::Crosshair | MouseCursor::Cell => SystemCursor::Crosshair,
        MouseCursor::EwResize
        | MouseCursor::EResize
        | MouseCursor::WResize
        | MouseCursor::ColResize => SystemCursor::SizeWE,
        MouseCursor::NsResize
        | MouseCursor::NResize
        | MouseCursor::SResize
        | MouseCursor::RowResize => SystemCursor::SizeNS,
        MouseCursor::NwseResize | MouseCursor::NwResize | MouseCursor::SeResize => {
            SystemCursor::SizeNWSE
        }
        MouseCursor::NeswResize | MouseCursor::NeResize | MouseCursor::SwResize => {
            SystemCursor::SizeNESW
        }
        MouseCursor::Move | MouseCursor::AllScroll => SystemCursor::SizeAll,
        MouseCursor::NotAllowed | MouseCursor::NoDrop => SystemCursor::No,
        _ => SystemCursor::Arrow,
    }
}
