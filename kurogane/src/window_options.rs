//! How an application window opens: the start window
//! ([`App::window`](crate::App::window)) and every window of
//! [`AppInstance::create_window`](crate::AppInstance::create_window).

use crate::runtime::BrowserBounds;

/// How an application window opens: its title, size and place, the size it
/// cannot be made smaller than, and its state.
///
/// Sizes and places are in density-independent pixels (DIP), which the
/// display's scale turns into pixels: 800 by 600 is 1200 by 900 pixels on a
/// display at 150%. A place is on the screen, from the top-left corner of
/// the primary display.
///
/// ```no_run
/// # use kurogane::{App, WindowOptions};
/// App::new("dist")
///     .window(WindowOptions::new().title("Notes").size(1100, 720).min_size(640, 480))
///     .run_or_exit();
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowOptions {
    title: Option<String>,
    size: Option<(u32, u32)>,
    bounds: Option<BrowserBounds>,
    min_size: Option<(u32, u32)>,
    state: WindowState,
}

impl WindowOptions {
    /// A window titled after its page, 800 by 600 centred on the primary
    /// display, shown normally.
    pub fn new() -> Self {
        Self::default()
    }

    /// The window's title, kept whatever title the page gives itself.
    /// Without one the window takes its page's title, as a browser tab
    /// does, so a page the window shows names it in the taskbar and the
    /// window switcher.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// The window's size, its frame included, centred on the primary
    /// display; made to fit the display's work area.
    /// [`bounds`](Self::bounds), when given, decides the size instead.
    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.size = Some((width, height));
        self
    }

    /// The window's place and size on the screen, its frame included: where
    /// it was the last time, to open it there again. A place no display
    /// shows any more is moved onto the nearest one, and a window larger
    /// than that display's work area is made to fit it. Wayland lets no
    /// application place its windows: there only the size applies.
    pub fn bounds(mut self, bounds: BrowserBounds) -> Self {
        self.bounds = Some(bounds);
        self
    }

    /// The size of content the user cannot make the window smaller than,
    /// its frame aside. A side of 0 leaves that side free.
    pub fn min_size(mut self, width: u32, height: u32) -> Self {
        self.min_size = Some((width, height));
        self
    }

    /// How the window first shows. Opened maximized, minimized or
    /// fullscreen, it restores to its size and place.
    pub fn state(mut self, state: WindowState) -> Self {
        self.state = state;
        self
    }

    pub(crate) fn fixed_title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub(crate) fn requested_size(&self) -> Option<(i32, i32)> {
        match self.bounds {
            Some(bounds) => Some((bounds.width, bounds.height)),
            None => self
                .size
                .map(|(width, height)| (width as i32, height as i32)),
        }
    }

    pub(crate) fn requested_bounds(&self) -> Option<BrowserBounds> {
        self.bounds
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
        if let Some((width, height)) = self.size {
            if width == 0 || height == 0 {
                return Some("a window's size is at least 1 by 1");
            }
            if !fits(width) || !fits(height) {
                return Some("a window's size is at most i32::MAX on each side");
            }
        }
        if let Some(bounds) = self.bounds
            && (bounds.width < 1 || bounds.height < 1)
        {
            return Some("a window's bounds are at least 1 by 1");
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

/// How a window first shows, and how it was when it closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
    Fullscreen,
    /// Not shown. A hidden window's browser keeps the application running
    /// as any open browser does.
    Hidden,
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

    fn bounds(width: i32, height: i32) -> BrowserBounds {
        BrowserBounds {
            x: 10,
            y: 20,
            width,
            height,
        }
    }

    #[test]
    fn bounds_decide_the_size_over_size() {
        let options = WindowOptions::new()
            .size(800, 600)
            .bounds(bounds(1000, 700));
        assert_eq!(options.requested_size(), Some((1000, 700)));
        assert_eq!(
            WindowOptions::new().size(800, 600).requested_size(),
            Some((800, 600))
        );
        assert_eq!(WindowOptions::new().requested_size(), None);
    }

    #[test]
    fn impossible_options_are_named() {
        let problem = |options: WindowOptions| options.problem();
        assert_eq!(problem(WindowOptions::new()), None);
        assert_eq!(
            problem(
                WindowOptions::new()
                    .title("x")
                    .size(800, 600)
                    .min_size(0, 600)
            ),
            None
        );
        assert!(problem(WindowOptions::new().size(0, 600)).is_some());
        assert!(problem(WindowOptions::new().size(u32::MAX, 600)).is_some());
        assert!(problem(WindowOptions::new().bounds(bounds(0, 600))).is_some());
        assert!(problem(WindowOptions::new().size(800, 600).min_size(900, 100)).is_some());
        assert!(
            problem(
                WindowOptions::new()
                    .bounds(bounds(800, 600))
                    .min_size(100, 601)
            )
            .is_some()
        );
        // A minimum alone is checked against nothing else
        assert_eq!(problem(WindowOptions::new().min_size(640, 480)), None);
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
