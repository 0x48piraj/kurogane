//! macOS-specific CEF initialization.

use cef::library_loader;
use objc2::{
    ClassType, MainThreadMarker, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObjectProtocol},
};
use objc2_app_kit::NSApp;

use crate::platform::macos::application::SimpleApplication;

/// Initializes the NSApplication subclass required by CEF on macOS.
///
/// This must be called before CEF initialization
pub fn init_ns_app() {
    // Load CEF frameworks
    let loader =
        library_loader::LibraryLoader::new(&std::env::current_exe().unwrap(), false);
    assert!(loader.load());

    // Ensure the correct NSApplication subclass is installed
    let mtm = MainThreadMarker::new().unwrap();

    unsafe {
        let _: Retained<AnyObject> =
            msg_send![SimpleApplication::class(), sharedApplication];
    }

    assert!(NSApp(mtm).isKindOfClass(SimpleApplication::class()));
}

mod application {
    use std::cell::Cell;

    use cef::application_mac::{
        CefAppProtocol, CrAppControlProtocol, CrAppProtocol,
    };
    use objc2::{DefinedClass, define_class, runtime::Bool};
    use objc2_app_kit::NSApplication;

    // Custom NSApplication subclass required by CEF on macOS
    pub struct SimpleApplicationIvars {
        handling_send_event: Cell<Bool>,
    }

    define_class! {
        #[unsafe(super(NSApplication))]
        #[ivars = SimpleApplicationIvars]
        pub struct SimpleApplication;

        unsafe impl CrAppControlProtocol for SimpleApplication {
            #[unsafe(method(setHandlingSendEvent:))]
            unsafe fn set_handling_send_event(&self, value: Bool) {
                self.ivars().handling_send_event.set(value);
            }
        }

        unsafe impl CrAppProtocol for SimpleApplication {
            #[unsafe(method(isHandlingSendEvent))]
            unsafe fn is_handling_send_event(&self) -> Bool {
                self.ivars().handling_send_event.get()
            }
        }

        unsafe impl CefAppProtocol for SimpleApplication {}
    }
}
