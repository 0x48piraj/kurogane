use std::time::Duration;

use kurogane::App;

fn main() {
    let kurogane = App::url("https://example.com")
        .start()
        .expect("Kurogane failed to initialize");

    let tick = Duration::from_millis(16); // optional

    while !kurogane.should_shutdown() {
        kurogane.pump();
        std::thread::sleep(tick);
    }

    kurogane.shutdown();
}
