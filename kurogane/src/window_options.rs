//! How an application window opens: the start window
//! ([`App::window`](crate::App::window)) and every window of
//! [`AppInstance::create_window`](crate::AppInstance::create_window); and
//! where one is, to open it there again ([`WindowPlacement`]).

use cef::Rect;
use serde::{Deserialize, Serialize};

/// How an application window opens: the name the application knows it by,
/// its title, size and place, the size it cannot be made smaller than, and
/// its state.
///
/// Sizes and places are in density-independent pixels (DIP), which the
/// display's scale turns into pixels: 800 by 600 is 1200 by 900 pixels on a
/// display at 150%. A place is on the screen, from the top-left corner of
/// the primary display.
///
/// A size and a place are the window's content, the area its page shows
/// in, without the frame the system draws around it. Centring a window and
/// keeping it on a display take the whole window, frame included, on
/// Windows and macOS. Under X11 the window manager adds the frame once the
/// window shows, so there the content is centred and kept on the display.
///
/// ```no_run
/// # use kurogane::{App, WindowOptions};
/// App::new("dist")
///     .window(WindowOptions::new().title("Notes").size(1100, 720).min_size(640, 480))
///     .run_or_exit();
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowOptions {
    name: Option<String>,
    title: Option<String>,
    size: Option<(u32, u32)>,
    /// Where [`placement`](Self::placement) puts the window; its state went
    /// to `state`, which a later [`state`](Self::state) replaces
    placement: Option<WindowPlacement>,
    min_size: Option<(u32, u32)>,
    state: WindowState,
}

impl WindowOptions {
    /// A window titled after its page, 800 by 600 centred on the primary
    /// display, shown normally.
    pub fn new() -> Self {
        Self::default()
    }

    /// The application's name for the window, never shown:
    /// [`WindowClosing::name`](crate::WindowClosing::name) gives it back as
    /// the window closes, to keep its placement under, and
    /// [`AppHandle::find_window_by_name`](crate::AppHandle::find_window_by_name)
    /// finds the window by it. One open window has a name at a time:
    /// [`AppInstance::create_window`](crate::AppInstance::create_window) is
    /// refused a name an open window has, until that window's close is
    /// reported.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// The window's title, kept whatever title the page gives itself.
    /// Without one the window takes its page's title, as a browser tab
    /// does, so a page the window shows names it in the taskbar and the
    /// window switcher.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// The size of the window's content, the window centred on the primary
    /// display and made to fit its work area.
    /// [`placement`](Self::placement), when given, decides the size instead.
    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.size = Some((width, height));
        self
    }

    /// Opens the window as `placement` says: where it was the last time
    /// ([`WindowClosing::placement`](crate::WindowClosing::placement)), at
    /// its size and in its state. It decides the size over
    /// [`size`](Self::size), and sets the state as [`state`](Self::state)
    /// does: of the two, the later call holds. A place no display shows any
    /// more is moved onto the nearest one, and a window larger than that
    /// display's work area is made to fit it. Wayland lets no application
    /// place its windows: there only the size applies.
    pub fn placement(mut self, placement: WindowPlacement) -> Self {
        self.state = placement.state;
        self.placement = Some(placement);
        self
    }

    /// The size of content the user cannot make the window smaller than.
    /// A side of 0 leaves that side free.
    pub fn min_size(mut self, width: u32, height: u32) -> Self {
        self.min_size = Some((width, height));
        self
    }

    /// How the window first shows. Opened maximized, minimized or
    /// fullscreen, it restores to its size and place. Sets the state
    /// [`placement`](Self::placement) sets: of the two, the later call
    /// holds.
    pub fn state(mut self, state: WindowState) -> Self {
        self.state = state;
        self
    }

    pub(crate) fn window_name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub(crate) fn fixed_title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    // Sides were checked against i32::MAX by `problem`, before any window
    // opens, so the casts below keep every value

    pub(crate) fn requested_size(&self) -> Option<(i32, i32)> {
        match self.placement {
            Some(placement) => Some((placement.width as i32, placement.height as i32)),
            None => self
                .size
                .map(|(width, height)| (width as i32, height as i32)),
        }
    }

    pub(crate) fn requested_bounds(&self) -> Option<Rect> {
        self.placement.map(|placement| Rect {
            x: placement.x,
            y: placement.y,
            width: placement.width as i32,
            height: placement.height as i32,
        })
    }

    pub(crate) fn minimum(&self) -> Option<(i32, i32)> {
        self.min_size
            .map(|(width, height)| (width as i32, height as i32))
    }

    pub(crate) fn initial_state(&self) -> WindowState {
        self.state
    }

    /// What makes these options impossible, if anything.
    pub(crate) fn problem(&self) -> Option<&'static str> {
        let fits = |side: u32| i32::try_from(side).is_ok();
        if self.name.as_deref() == Some("") {
            return Some("a window's name is not empty");
        }
        if let Some((width, height)) = self.size {
            if width == 0 || height == 0 {
                return Some("a window's size is at least 1 by 1");
            }
            if !fits(width) || !fits(height) {
                return Some("a window's size is at most i32::MAX on each side");
            }
        }
        if let Some(placement) = self.placement {
            if placement.width == 0 || placement.height == 0 {
                return Some("a window's placement is at least 1 by 1");
            }
            if !fits(placement.width) || !fits(placement.height) {
                return Some("a window's placement is at most i32::MAX on each side");
            }
        }
        if let Some((width, height)) = self.min_size {
            if !fits(width) || !fits(height) {
                return Some("a window's minimum size is at most i32::MAX on each side");
            }
            if let Some((w, h)) = self.requested_size()
                && (width as i32 > w || height as i32 > h)
            {
                return Some("a window's minimum size is larger than its size");
            }
        }
        None
    }
}

/// Where a window's content is on the screen, in density-independent
/// pixels, and how the window shows. A window maximized, minimized or
/// fullscreen is placed where it restores to.
///
/// [`WindowClosing::placement`](crate::WindowClosing::placement) gives one
/// as a window closes, and [`WindowOptions::placement`] opens a window as
/// one says. Serialized with serde it is
/// `{"x":200,"y":150,"width":900,"height":600,"state":"Maximized"}` in JSON,
/// to keep with the application's settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowPlacement {
    /// The content's left edge, from the primary display's top-left corner.
    pub x: i32,
    /// The content's top edge.
    pub y: i32,
    /// The content's width.
    pub width: u32,
    /// The content's height.
    pub height: u32,
    /// How it shows.
    pub state: WindowState,
}

/// How a window shows: first, as its options say, and as it closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum WindowState {
    /// Shown normally.
    #[default]
    Normal,
    /// Minimized.
    Minimized,
    /// Maximized.
    Maximized,
    /// Filling its display, without a frame.
    ///
    /// Under X11 a window opened in this state shows at its size, because
    /// Chromium drops a state set before the window maps.
    Fullscreen,
    /// Not shown. A hidden window's browser keeps the application running
    /// as any open browser does.
    Hidden,
}

impl WindowState {
    /// The state a window in this one shows in once restored: minimized or
    /// hidden, it comes back normal.
    pub(crate) fn restored(self) -> Self {
        match self {
            Self::Normal | Self::Minimized | Self::Hidden => Self::Normal,
            Self::Maximized => Self::Maximized,
            Self::Fullscreen => Self::Fullscreen,
        }
    }
}

impl From<WindowState> for cef::ShowState {
    fn from(state: WindowState) -> Self {
        match state {
            WindowState::Normal => cef::ShowState::NORMAL,
            WindowState::Minimized => cef::ShowState::MINIMIZED,
            WindowState::Maximized => cef::ShowState::MAXIMIZED,
            WindowState::Fullscreen => cef::ShowState::FULLSCREEN,
            WindowState::Hidden => cef::ShowState::HIDDEN,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placement(width: u32, height: u32) -> WindowPlacement {
        WindowPlacement {
            x: 10,
            y: 20,
            width,
            height,
            state: WindowState::Normal,
        }
    }

    #[test]
    fn a_placement_decides_the_size_over_size() {
        let options = WindowOptions::new()
            .size(800, 600)
            .placement(placement(1000, 700));
        assert_eq!(options.requested_size(), Some((1000, 700)));
        assert_eq!(
            WindowOptions::new().size(800, 600).requested_size(),
            Some((800, 600))
        );
        assert_eq!(WindowOptions::new().requested_size(), None);
    }

    #[test]
    fn of_a_placement_and_a_state_the_later_holds() {
        let maximized = WindowPlacement {
            state: WindowState::Maximized,
            ..placement(900, 600)
        };
        let options = WindowOptions::new().placement(maximized);
        assert_eq!(options.initial_state(), WindowState::Maximized);
        let options = options.state(WindowState::Hidden);
        assert_eq!(options.initial_state(), WindowState::Hidden);
        let options = options.placement(maximized);
        assert_eq!(options.initial_state(), WindowState::Maximized);
    }

    #[test]
    fn impossible_options_are_named() {
        let problem = |options: WindowOptions| options.problem();
        assert_eq!(problem(WindowOptions::new()), None);
        assert_eq!(
            problem(
                WindowOptions::new()
                    .name("main")
                    .title("x")
                    .size(800, 600)
                    .min_size(0, 600)
            ),
            None
        );
        assert!(problem(WindowOptions::new().name("")).is_some());
        assert!(problem(WindowOptions::new().size(0, 600)).is_some());
        assert!(problem(WindowOptions::new().size(u32::MAX, 600)).is_some());
        assert!(problem(WindowOptions::new().placement(placement(0, 600))).is_some());
        assert!(problem(WindowOptions::new().placement(placement(800, u32::MAX))).is_some());
        assert!(problem(WindowOptions::new().size(800, 600).min_size(900, 100)).is_some());
        assert!(
            problem(
                WindowOptions::new()
                    .placement(placement(800, 600))
                    .min_size(100, 601)
            )
            .is_some()
        );
        // A minimum alone is checked against nothing else
        assert_eq!(problem(WindowOptions::new().min_size(640, 480)), None);
    }

    #[test]
    fn a_placement_keeps_its_json_shape() {
        let maximized = WindowPlacement {
            x: -1200,
            y: 150,
            width: 900,
            height: 600,
            state: WindowState::Maximized,
        };
        let json = serde_json::to_string(&maximized).unwrap();
        assert_eq!(
            json,
            r#"{"x":-1200,"y":150,"width":900,"height":600,"state":"Maximized"}"#
        );
        assert_eq!(
            serde_json::from_str::<WindowPlacement>(&json).unwrap(),
            maximized
        );
    }

    #[test]
    fn a_window_minimized_or_hidden_restores_normal() {
        for (state, restored) in [
            (WindowState::Normal, WindowState::Normal),
            (WindowState::Minimized, WindowState::Normal),
            (WindowState::Maximized, WindowState::Maximized),
            (WindowState::Fullscreen, WindowState::Fullscreen),
            (WindowState::Hidden, WindowState::Normal),
        ] {
            assert_eq!(state.restored(), restored, "{state:?}");
        }
    }

    #[test]
    fn every_state_has_its_show_state() {
        for (state, shown) in [
            (WindowState::Normal, cef::ShowState::NORMAL),
            (WindowState::Minimized, cef::ShowState::MINIMIZED),
            (WindowState::Maximized, cef::ShowState::MAXIMIZED),
            (WindowState::Fullscreen, cef::ShowState::FULLSCREEN),
            (WindowState::Hidden, cef::ShowState::HIDDEN),
        ] {
            assert_eq!(cef::ShowState::from(state), shown, "{state:?}");
        }
    }
}
