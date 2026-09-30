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

    // Starts hidden, and the application never shows it (a second launch
    // would: it brings every window to the front). Once the other windows
    // close, its browser keeps the application running, as any open browser
    // does; Ctrl+C ends it
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

    runtime.run().expect("Kurogane failed to run");
}
