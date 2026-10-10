//! What the application hears as one of its windows closes:
//! [`App::on_window_closing`](crate::App::on_window_closing).

use std::panic::{AssertUnwindSafe, catch_unwind};

use tanso::{ImplView, ImplWindow, Rect, Window};
use tracing::{debug, error};

use crate::runtime::AppHandle;
use crate::window_options::{WindowPlacement, WindowState};
use crate::window_registry::WindowId;

/// A window of the application's closing, passed to
/// [`App::on_window_closing`](crate::App::on_window_closing): which it is,
/// and where it was, to open it there again with
/// [`WindowOptions::placement`](crate::WindowOptions::placement).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowClosing {
    window: WindowId,
    name: Option<String>,
    placement: WindowPlacement,
}

impl WindowClosing {
    /// The window: the one
    /// [`AppInstance::create_window`](crate::AppInstance::create_window)
    /// returned, or the start window.
    pub fn window(&self) -> WindowId {
        self.window
    }

    /// The name its options gave it
    /// ([`WindowOptions::name`](crate::WindowOptions::name)); none for a
    /// window opened without one, as a page's is. The name is free from
    /// this report on: a window opened now may take it.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Where the window was and how it showed. A window maximized,
    /// minimized or fullscreen gives the place and size it restores to, not
    /// the display it fills. The state is `Normal`, `Maximized` or
    /// `Fullscreen`: a window minimized, or never shown, gives the state it
    /// shows in once restored, so the placement given back opens a window
    /// the user sees.
    ///
    /// Under xfwm4 a window the user maximized can give its maximized place
    /// and size as `Normal` because xfwm4 sends a maximize's geometry before
    /// its state.
    pub fn placement(&self) -> WindowPlacement {
        self.placement
    }
}

/// An application window whose browser has closed, as the registry had it.
pub(crate) struct Closing {
    pub(crate) id: WindowId,
    /// Its name, which the registry has just let go
    pub(crate) name: Option<String>,
    pub(crate) window: Window,
    /// Where it restores to, as its bounds callbacks said
    pub(crate) restored: Option<Rect>,
    /// The state it showed in when last not minimized
    pub(crate) shown: WindowState,
}

/// Tells the application's hook that `closing`'s window closes. UI thread,
/// from the browser's OnBeforeClose, which every way a window closes goes
/// through while the window still answers, before the last one ends the
/// application. Call it with no registry guard held: the hook may read the
/// application's state. A hook that panics is logged, and the window closes
/// as it would.
pub(crate) fn report(app: &AppHandle, closing: Closing) {
    let Some(hooks) = app.hooks() else {
        return;
    };
    let Some(hook) = hooks.window_closing.as_ref() else {
        return;
    };
    let window = &closing.window;
    // A minimized window comes back as it showed before. One CEF opened
    // hidden is minimized too (Windows), and comes back as it opened
    let state = if window.is_minimized() != 0 {
        closing.shown
    } else if window.is_fullscreen() != 0 {
        WindowState::Fullscreen
    } else if window.is_maximized() != 0 {
        WindowState::Maximized
    } else {
        WindowState::Normal
    };
    let Rect {
        x,
        y,
        width,
        height,
    } = closing
        .restored
        .unwrap_or_else(|| window.bounds_in_screen());
    // A window's sides are positive; at least 1 keeps the placement one a
    // window can open at
    let placement = WindowPlacement {
        x,
        y,
        width: width.max(1).unsigned_abs(),
        height: height.max(1).unsigned_abs(),
        state,
    };
    debug!(
        "[Window] {} closing at {x},{y} {width}x{height}, {state:?}{}",
        closing.id.as_u32(),
        closing
            .name
            .as_deref()
            .map(|name| format!(", named {name}"))
            .unwrap_or_default()
    );
    let event = WindowClosing {
        window: closing.id,
        name: closing.name,
        placement,
    };
    if catch_unwind(AssertUnwindSafe(|| hook(&event, app))).is_err() {
        error!("on_window_closing panicked; the window closes as it would");
    }
}
