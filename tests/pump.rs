use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use kurogane::{
    GpuMode, IpcDispatcher, Runtime, RuntimeHandle,
};

fn main() {
    let dispatcher = Arc::new(IpcDispatcher::new(
        HashMap::new(),
        HashMap::new(),
    ));

    let handle = Runtime::start(
        "https://example.com".to_string(),
        None,
        dispatcher,
        None,
        false,
        GpuMode::Hardware,
        vec![],
    )
    .expect("CEF failed to initialize");

    pump_until_shutdown(&handle);
    handle.shutdown();
}

fn pump_until_shutdown(handle: &RuntimeHandle) {
    let tick = Duration::from_millis(16);

    while !handle.should_shutdown() {
        handle.pump();
        std::thread::sleep(tick);
    }
}
