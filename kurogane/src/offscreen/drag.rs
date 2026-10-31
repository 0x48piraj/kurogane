//! In-page drags of an offscreen browser.
//!
//! A page starts a drag (CEF's StartDragging) while a button is held. Kurogane
//! runs it from the host's pointer calls as its target and its source, as the
//! OS's drag loop does for a windowed browser. The pointer's moves and its
//! release drive the drag in place of mouse events. The drag stays inside the
//! page.

use std::cell::RefCell;
use std::collections::HashMap;

use tanso::*;
use tracing::debug;

use super::paint::PainterKey;

thread_local! {
    /// Drags under way in this thread's offscreen browsers. Only CEF's UI
    /// thread has any.
    static DRAGS: RefCell<HashMap<PainterKey, Drag>> = RefCell::new(HashMap::new());
}

/// A drag under way.
struct Drag {
    /// Writable copy of the page's drag data.
    data: DragData,
    allowed: DragOperationsMask,
    /// Operation the page last accepted.
    operation: DragOperationsMask,
    /// Pointer over the view, where the drag has entered.
    inside: bool,
}

/// Starts the drag of `data` the page of the browser of `key` began, entering
/// at `event` once CEF's StartDragging returns. Returns whether Kurogane runs
/// it.
pub(crate) fn start(
    key: PainterKey,
    browser: &Browser,
    data: &DragData,
    allowed: DragOperationsMask,
    event: MouseEvent,
) -> bool {
    let Some(data) = data.copy() else {
        return false;
    };
    let drag = Drag {
        data,
        allowed,
        operation: DragOperationsMask::NONE,
        inside: false,
    };
    DRAGS.with_borrow_mut(|drags| drags.insert(key, drag));
    // CEF holds the drag data's lock through StartDragging
    let mut task = EnterTask::new(browser.clone(), key, event);
    if post_task(ThreadId::UI, &mut task) == 0 {
        debug!("CEF refused to start an offscreen browser's drag");
        forget(key);
        return false;
    }
    true
}

/// Notes `operation` as the one the page accepts for the drag of `key`.
pub(crate) fn accept(key: PainterKey, operation: DragOperationsMask) {
    DRAGS.with_borrow_mut(|drags| {
        if let Some(drag) = drags.get_mut(&key) {
            drag.operation = operation;
        }
    });
}

/// Moves the drag of `key` to `event`, out of the view when `leave`. Returns
/// whether a drag took the move.
pub(crate) fn moved(key: PainterKey, host: &BrowserHost, event: &MouseEvent, leave: bool) -> bool {
    // Out of the storage first; CEF's calls may reach it again
    let state = DRAGS.with_borrow(|drags| {
        drags
            .get(&key)
            .map(|drag| (drag.data.clone(), drag.allowed, drag.inside))
    });
    let Some((mut data, allowed, inside)) = state else {
        return false;
    };
    match (leave, inside) {
        (true, true) => host.drag_target_drag_leave(),
        (true, false) => {}
        (false, true) => host.drag_target_drag_over(event, allowed),
        (false, false) => host.drag_target_drag_enter(&mut data, event, allowed),
    }
    DRAGS.with_borrow_mut(|drags| {
        if let Some(drag) = drags.get_mut(&key) {
            drag.inside = !leave;
        }
    });
    true
}

/// Drops the drag of `key` at `event` and ends it, nowhere when the pointer is
/// out of the view. Returns whether a drag took the release.
pub(crate) fn released(key: PainterKey, host: &BrowserHost, event: &MouseEvent) -> bool {
    let Some(drag) = DRAGS.with_borrow_mut(|drags| drags.remove(&key)) else {
        return false;
    };
    let operation = if drag.inside {
        host.drag_target_drop(event);
        drag.operation
    } else {
        DragOperationsMask::NONE
    };
    end(host, event, operation);
    true
}

/// Ends the drag of `key` at `event` without a drop, if one is under way.
pub(crate) fn cancel(key: PainterKey, host: &BrowserHost, event: &MouseEvent) {
    let Some(drag) = DRAGS.with_borrow_mut(|drags| drags.remove(&key)) else {
        return;
    };
    if drag.inside {
        host.drag_target_drag_leave();
    }
    end(host, event, DragOperationsMask::NONE);
}

/// Tells the page's drag source that its drag ended with `operation` at `event`.
fn end(host: &BrowserHost, event: &MouseEvent, operation: DragOperationsMask) {
    host.drag_source_ended_at(event.x, event.y, operation);
    host.drag_source_system_drag_ended();
}

/// Forgets the drag of `key`, a browser that closed.
pub(crate) fn forget(key: PainterKey) {
    // Dropped outside the borrow
    let drag = DRAGS.with_borrow_mut(|drags| drags.remove(&key));
    drop(drag);
}

// Enters a drag the page started, after CEF's StartDragging returns
wrap_task! {
    struct EnterTask {
        browser: Browser,
        key: PainterKey,
        event: MouseEvent,
    }

    impl Task {
        fn execute(&self) {
            if let Some(host) = self.browser.host() {
                moved(self.key, &host, &self.event, false);
            }
        }
    }
}
