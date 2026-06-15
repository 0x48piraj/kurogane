fn main() {
    let runtime = kurogane::App::url("https://xkcd.com")
        .start()
        .unwrap();

    // Visible immediately
    runtime.create_window(kurogane::WindowOptions {
        url: "https://en.wikipedia.org/wiki/Rust_(programming_language)".into(),
        bounds: kurogane::BrowserBounds {
            x: 120,
            y: 90,
            width: 800,
            height: 600,
        },
        show_state: kurogane::WindowState::Normal,
    }).unwrap();

    // Starts maximized
    runtime.create_window(kurogane::WindowOptions {
        url: "https://github.com/0x48piraj/kurogane".into(),
        bounds: kurogane::BrowserBounds {
            x: 240,
            y: 180,
            width: 800,
            height: 600,
        },
        show_state: kurogane::WindowState::Maximized,
    }).unwrap();

    // Starts minimized
    runtime.create_window(kurogane::WindowOptions {
        url: "https://www.rust-lang.org".into(),
        bounds: kurogane::BrowserBounds {
            x: 360,
            y: 270,
            width: 800,
            height: 600,
        },
        show_state: kurogane::WindowState::Minimized,
    }).unwrap();

    // Starts hidden
    runtime.create_window(kurogane::WindowOptions {
        url: "https://docs.rs".into(),
        bounds: kurogane::BrowserBounds {
            x: 480,
            y: 360,
            width: 800,
            height: 600,
        },
        show_state: kurogane::WindowState::Hidden,
    }).unwrap();

    while !runtime.should_shutdown() {
        runtime.pump();
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
}
