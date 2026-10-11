//! macOS-specific CEF initialization.

use std::sync::{OnceLock, Weak};

use objc2::{
    ClassType, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject, Sel},
    sel,
};
use objc2_app_kit::{
    NSApp, NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate,
    NSApplicationTerminateReply, NSEventModifierFlags, NSMenu, NSMenuItem, NSRunningApplication,
};
use objc2_foundation::NSString;

use crate::error::RuntimeError;
use crate::platform::macos::application::SimpleApplication;
use crate::runtime::{AppHandle, RuntimeServices};

/// The application, for the Objective-C `terminate:` override. Held weakly:
/// the static lasts as long as the process and must not keep the
/// application's state alive once it has ended.
static APP: OnceLock<Weak<RuntimeServices>> = OnceLock::new();

/// Registers the application for the `terminate:` override, once CEF has
/// initialized.
pub fn set_app(app: &AppHandle) {
    let first = APP.set(app.downgrade()).is_ok();
    debug_assert!(first, "CEF initializes once per process");
}

/// Loads CEF and, in the browser process, installs the required
/// `NSApplication` subclass and makes an unbundled process a regular
/// foreground app.
///
/// Loads the CEF the runtime resolves, its bundle's or the one tanso finds.
///
/// CEF's subprocesses (`--type=renderer`, `gpu-process`, `utility`) get the
/// library alone: an `NSApplication` registers its process with LaunchServices
/// as an application, and inside a bundle every subprocess would then own a
/// Dock tile of its own. CEF's helper executables install none.
///
/// Under [`SandboxMode::Chromium`](crate::SandboxMode::Chromium) subprocesses
/// enter the seatbelt sandbox before the framework is loaded.
///
/// Must run on the main thread before CEF initialization. Returns the
/// directory of the CEF it loaded.
pub fn init_ns_app(sandbox: crate::SandboxMode) -> Result<std::path::PathBuf, RuntimeError> {
    let browser = crate::runtime::is_browser_process();

    if !browser && matches!(sandbox, crate::SandboxMode::Chromium) {
        crate::sandbox::macos::initialize_helper()?;
    }

    let cef_root = crate::runtime::load_libcef()?;

    if !browser {
        return Ok(cef_root);
    }

    let mtm = MainThreadMarker::new().expect("init_ns_app must run on the main thread");

    // SAFETY: `+sharedApplication` is a valid zero-arg class method on
    // `SimpleApplication` (an `NSApplication` subclass). Main-thread
    // invocation is verified above, and `Retained` safely manages
    // ownership of the returned instance.
    unsafe {
        let _: Retained<AnyObject> = msg_send![SimpleApplication::class(), sharedApplication];
    }

    let app = NSApp(mtm);
    assert!(app.isKindOfClass(SimpleApplication::class()));

    promote_unbundled(&app);

    Ok(cef_root)
}

/// Ensures unbundled browser processes use a foreground activation policy.
///
/// Bundled applications retain the policy declared by their `Info.plist`.
fn promote_unbundled(app: &NSApplication) {
    if app.activationPolicy() == NSApplicationActivationPolicy::Prohibited {
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    }
}

/// Installs the application delegate for the process lifetime.
///
/// The delegate must be installed on the main thread after CEF initialization.
pub fn setup_app_delegate() {
    let mtm = MainThreadMarker::new().expect("Not running on the main thread");
    let app = NSApp(mtm);
    assert!(app.isKindOfClass(SimpleApplication::class()));

    let delegate = SimpleAppDelegate::new(mtm);
    let delegate_proto =
        ProtocolObject::<dyn NSApplicationDelegate>::from_retained(delegate.clone());
    app.setDelegate(Some(&delegate_proto));

    assert!(
        app.delegate()
            .unwrap()
            .isKindOfClass(SimpleAppDelegate::class())
    );

    // NSApplication does not retain its delegate. Keep the retained handle alive
    // until process exit so it outlives CEF initialization
    std::mem::forget(delegate);
}

/// Installs the standard App, Edit and Window menus when no menu exists.
/// Existing menus are preserved.
///
/// Must run on the main thread after CEF initialization.
pub fn install_default_menu() {
    let mtm = MainThreadMarker::new().expect("install_default_menu must run on the main thread");
    let app = NSApp(mtm);
    if app.mainMenu().is_some_and(|menu| menu.numberOfItems() > 0) {
        return;
    }

    let name = app_name();
    let command = NSEventModifierFlags::Command;
    let shift = command | NSEventModifierFlags::Shift;
    let option = command | NSEventModifierFlags::Option;
    let bar = NSMenu::new(mtm);

    let app_menu = submenu(mtm, &bar, &name);
    let about = format!("About {name}");
    add_item(
        &app_menu,
        &about,
        sel!(orderFrontStandardAboutPanel:),
        "",
        command,
    );
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    add_item(
        &app_menu,
        &format!("Hide {name}"),
        sel!(hide:),
        "h",
        command,
    );
    add_item(
        &app_menu,
        "Hide Others",
        sel!(hideOtherApplications:),
        "h",
        option,
    );
    add_item(
        &app_menu,
        "Show All",
        sel!(unhideAllApplications:),
        "",
        command,
    );
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    add_item(
        &app_menu,
        &format!("Quit {name}"),
        sel!(terminate:),
        "q",
        command,
    );

    let edit = submenu(mtm, &bar, "Edit");
    add_item(&edit, "Undo", sel!(undo:), "z", command);
    add_item(&edit, "Redo", sel!(redo:), "z", shift);
    edit.addItem(&NSMenuItem::separatorItem(mtm));
    add_item(&edit, "Cut", sel!(cut:), "x", command);
    add_item(&edit, "Copy", sel!(copy:), "c", command);
    add_item(&edit, "Paste", sel!(paste:), "v", command);
    let match_style = option | NSEventModifierFlags::Shift;
    add_item(
        &edit,
        "Paste and Match Style",
        sel!(pasteAndMatchStyle:),
        "v",
        match_style,
    );
    add_item(&edit, "Delete", sel!(delete:), "", command);
    add_item(&edit, "Select All", sel!(selectAll:), "a", command);

    let window = submenu(mtm, &bar, "Window");
    add_item(&window, "Minimize", sel!(performMiniaturize:), "m", command);
    add_item(&window, "Zoom", sel!(performZoom:), "", command);
    window.addItem(&NSMenuItem::separatorItem(mtm));
    add_item(&window, "Close", sel!(performClose:), "w", command);
    add_item(
        &window,
        "Bring All to Front",
        sel!(arrangeInFront:),
        "",
        command,
    );
    // AppKit populates the Window menu with the application's windows
    app.setWindowsMenu(Some(&window));

    app.setMainMenu(Some(&bar));
}

/// Quits as the application menu's Quit does.
/// Sends `terminate:` after the current event, allowing orderly
/// shutdown to run outside the caller.
pub fn quit() {
    let Some(mtm) = MainThreadMarker::new() else {
        tracing::debug!("quit asked off the main thread; ignored");
        return;
    };
    let app = NSApp(mtm);
    // SAFETY: NSObject's `performSelector:withObject:afterDelay:` takes a
    // selector, an object (nil) and an `NSTimeInterval` (f64); `terminate:`
    // takes its one object argument
    unsafe {
        let _: () = msg_send![
            &*app,
            performSelector: sel!(terminate:),
            withObject: None::<&AnyObject>,
            afterDelay: 0.0f64
        ];
    }
}

/// Returns the application display name, falling back to the executable name,
/// then to "Application" so no item reads "Quit " or "About ".
fn app_name() -> String {
    let localized = NSRunningApplication::currentApplication()
        .localizedName()
        .map(|name| name.to_string());
    let stem = || {
        std::env::current_exe().ok().and_then(|exe| {
            exe.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
    };
    menu_name(localized, stem)
}

/// The first non-empty of `localized` and `stem`, else "Application".
fn menu_name(localized: Option<String>, stem: impl FnOnce() -> Option<String>) -> String {
    localized
        .filter(|name| !name.trim().is_empty())
        .or_else(|| stem().filter(|name| !name.trim().is_empty()))
        .unwrap_or_else(|| "Application".to_owned())
}

/// Adds a titled submenu to `bar`.
fn submenu(mtm: MainThreadMarker, bar: &NSMenu, title: &str) -> Retained<NSMenu> {
    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
    let holder = NSMenuItem::new(mtm);
    holder.setSubmenu(Some(&menu));
    bar.addItem(&holder);
    menu
}

/// Adds an untargeted menu item with the given action and key equivalent.
fn add_item(menu: &NSMenu, title: &str, action: Sel, key: &str, modifiers: NSEventModifierFlags) {
    // SAFETY: Standard AppKit actions are dispatched through the responder chain.
    let item = unsafe {
        menu.addItemWithTitle_action_keyEquivalent(
            &NSString::from_str(title),
            Some(action),
            &NSString::from_str(key),
        )
    };
    item.setKeyEquivalentModifierMask(modifiers);
}

define_class! {
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    pub struct SimpleAppDelegate;

    unsafe impl NSObjectProtocol for SimpleAppDelegate {}

    unsafe impl NSApplicationDelegate for SimpleAppDelegate {
        #[unsafe(method(applicationShouldTerminate:))]
        unsafe fn application_should_terminate(&self, _sender: &NSApplication) -> NSApplicationTerminateReply {
            NSApplicationTerminateReply::TerminateNow
        }

        /// Ignores dock reopen requests while the application is running.
        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        unsafe fn application_should_handle_reopen(&self, _sender: &NSApplication, _has_visible_windows: Bool) -> Bool {
            Bool::NO
        }

        /// Enables secure state restoration encoding.
        ///
        /// Prevents macOS from restoring stale windows after an unclean shutdown.
        #[unsafe(method(applicationSupportsSecureRestorableState:))]
        unsafe fn application_supports_secure_restorable_state(&self, _sender: &NSApplication) -> Bool {
            Bool::YES
        }
    }
}

impl SimpleAppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = SimpleAppDelegate::alloc(mtm).set_ivars(());
        // SAFETY: `this` is a freshly allocated instance with initialized ivars.
        // Invoking `-init` via `super` correctly executes `NSObject`'s
        // zero-arg designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

mod application {
    use std::cell::Cell;

    use tanso::application_mac::{CefAppProtocol, CrAppControlProtocol, CrAppProtocol};
    use objc2::{
        DefinedClass, define_class, extern_methods, msg_send,
        runtime::{AnyObject, Bool},
    };
    use objc2_app_kit::{NSApplication, NSEvent};

    use super::APP;
    use crate::runtime::{AppHandle, Close};

    /// CEF-compatible `NSApplication` subclass.
    #[derive(Default)]
    pub struct SimpleApplicationIvars {
        handling_send_event: Cell<Bool>,
    }

    define_class! {
        #[unsafe(super(NSApplication))]
        #[ivars = SimpleApplicationIvars]
        pub struct SimpleApplication;

        impl SimpleApplication {
            #[unsafe(method(sendEvent:))]
            unsafe fn send_event(&self, event: &NSEvent) {
                let was_sending_event = self.is_handling_send_event();
                if !was_sending_event {
                    self.set_handling_send_event(true);
                }

                let _: () = msg_send![super(self), sendEvent:event];

                if !was_sending_event {
                    self.set_handling_send_event(false);
                }
            }

            /// Converts application termination into orderly browser shutdown.
            ///
            /// Cocoa's default `terminate:` implementation exits the process,
            /// which prevents CEF from leaving the run loop and completing shutdown.
            /// Closing all browsers instead lets the normal CEF shutdown path run.
            #[unsafe(method(terminate:))]
            unsafe fn terminate(&self, _sender: &AnyObject) {
                // Unload handlers still run, as for a window closed by hand.
                // This is the main thread, CEF's UI thread, so it closes at once
                // Nothing to close once the application has ended
                if let Some(app) = APP.get().and_then(AppHandle::upgrade) {
                    app.request(Close::Everything { force: false });
                }
            }
        }

        unsafe impl CrAppControlProtocol for SimpleApplication {
            #[unsafe(method(setHandlingSendEvent:))]
            unsafe fn _set_handling_send_event(&self, value: Bool) {
                self.ivars().handling_send_event.set(value);
            }
        }

        unsafe impl CrAppProtocol for SimpleApplication {
            #[unsafe(method(isHandlingSendEvent))]
            unsafe fn _is_handling_send_event(&self) -> Bool {
                self.ivars().handling_send_event.get()
            }
        }

        unsafe impl CefAppProtocol for SimpleApplication {}
    }

    impl SimpleApplication {
        extern_methods! {
            #[unsafe(method(setHandlingSendEvent:))]
            fn set_handling_send_event(&self, handling_send_event: bool);

            #[unsafe(method(isHandlingSendEvent))]
            fn is_handling_send_event(&self) -> bool;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::menu_name;

    #[test]
    fn the_menu_names_the_application_never_nothing() {
        let stem = |s: &str| {
            let s = s.to_owned();
            move || Some(s)
        };
        assert_eq!(menu_name(Some("Notes".into()), stem("notes")), "Notes");
        assert_eq!(menu_name(None, stem("notes")), "notes");
        assert_eq!(menu_name(Some("  ".into()), stem("notes")), "notes");
        assert_eq!(menu_name(None, || None), "Application");
        assert_eq!(menu_name(Some(String::new()), stem("")), "Application");
    }
}
