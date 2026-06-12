fn main() {
    let runtime = kurogane::App::url("https://xkcd.com")
        .start()
        .unwrap();

    runtime.create_window(
        kurogane::WindowOptions {
            url: "https://en.wikipedia.org/wiki/Rust_(programming_language)".into(),
            bounds: kurogane::BrowserBounds {
                x: 120,
                y: 90,
                width: 800,
                height: 600,
            },
        }
    ).unwrap();

    runtime.create_window(
        kurogane::WindowOptions {
            url: "https://github.com/0x48piraj/kurogane".into(),
            bounds: kurogane::BrowserBounds {
                x: 240,
                y: 180,
                width: 800,
                height: 600,
            },
        }
    ).unwrap();

    while !runtime.should_shutdown() {
        runtime.pump();
        std::thread::sleep(
            std::time::Duration::from_millis(16)
        );
    }
}
