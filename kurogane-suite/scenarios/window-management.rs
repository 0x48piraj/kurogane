fn main() {
    let runtime = kurogane::App::url("https://xkcd.com")
        .start()
        .expect("Kurogane failed to initialize");

    // Visible immediately
    runtime
        .create_window(kurogane::WindowOptions {
            url: "https://en.wikipedia.org/wiki/Rust_(programming_language)".into(),
            bounds: kurogane::BrowserBounds {
                x: 120,
                y: 90,
                width: 800,
                height: 600,
            },
            show_state: kurogane::WindowState::Normal,
        })
        .expect("failed to create browser window");

    // Starts maximized
    runtime
        .create_window(kurogane::WindowOptions {
            url: "https://github.com/0x48piraj/kurogane".into(),
            bounds: kurogane::BrowserBounds {
                x: 240,
                y: 180,
                width: 800,
                height: 600,
            },
            show_state: kurogane::WindowState::Maximized,
        })
        .expect("failed to create browser window");

    // Starts minimized
    runtime
        .create_window(kurogane::WindowOptions {
            url: "https://www.rust-lang.org".into(),
            bounds: kurogane::BrowserBounds {
                x: 360,
                y: 270,
                width: 800,
                height: 600,
            },
            show_state: kurogane::WindowState::Minimized,
        })
        .expect("failed to create browser window");

    // Starts hidden
    // TODO: Verify hidden-window shutdown behavior.
    // On Windows a hidden browser may continue running after
    // all visible windows close, preventing runtime shutdown.
    runtime
        .create_window(kurogane::WindowOptions {
            url: "https://docs.rs".into(),
            bounds: kurogane::BrowserBounds {
                x: 480,
                y: 360,
                width: 800,
                height: 600,
            },
            show_state: kurogane::WindowState::Hidden,
        })
        .expect("failed to create browser window");

    while !runtime.should_shutdown() {
        runtime.pump();
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
}
