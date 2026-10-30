//! Chromium offscreen, its frames drawn by SDL3 in a window SDL3 makes.
//!
//! The browser has no window. Each frame comes to a callback inside
//! Kurogane's pump on the main thread. The callback keeps what changed in a
//! buffer the loop shares through an `Rc`. The loop draws that buffer through
//! a streaming texture. The window's size in pixels and its display scale go
//! to the browser as they change. The page's cursor comes as an event.
//!
//! The page takes no input from this host yet.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use kurogane::{MouseCursor, OffscreenEvent, OffscreenOptions, PixelFrame};
use kurogane_suite::embedding;
use sdl3::event::{Event, EventSender, WindowEvent};
use sdl3::mouse::{Cursor, SystemCursor};
use sdl3::pixels::PixelFormat;
use sdl3::render::{FRect, Texture, TextureCreator};
use sdl3::video::{Window, WindowContext};

/// User event that wakes SDL's loop.
struct Wake;

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
        .window("Kurogane offscreen in SDL3", 1000, 700)
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

    let mut texture: Option<Texture<'_>> = None;
    // Kept while it shows; SDL shows a cursor only while its handle lives
    let mut cursor: Option<Cursor>;
    let mut pump = sdl.event_pump().expect("SDL has an event pump");
    while !runtime.should_shutdown() {
        let first = pump.wait_event_timeout_ms(schedule.wait_ms());
        for event in first.into_iter().chain(pump.poll_iter()) {
            match event {
                Event::Window {
                    win_event: WindowEvent::CloseRequested,
                    ..
                }
                | Event::Quit { .. } => runtime.handle().close_all_browsers(true),
                // A move to a display of another scale changes the pixel size
                Event::Window {
                    win_event: WindowEvent::PixelSizeChanged(..) | WindowEvent::DisplayChanged(..),
                    ..
                } => {
                    let (width, height) = canvas.window().size_in_pixels();
                    browser.set_size(width, height);
                    browser.set_scale(scale(canvas.window()));
                }
                _ => {}
            }
        }
        schedule.pump_if_due(&runtime);

        for event in browser.events() {
            match event {
                OffscreenEvent::Cursor(shape) => {
                    cursor = Cursor::from_system(system_cursor(&shape)).ok();
                    if let Some(cursor) = &cursor {
                        cursor.set();
                    }
                }
                OffscreenEvent::Tooltip(Some(text)) => println!("tooltip: {text}"),
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
