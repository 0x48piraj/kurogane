use kurogane::{WindowOptions, WindowPlacement, WindowState};

fn main() {
    kurogane_suite::logging();
    let runtime = kurogane::App::url("https://xkcd.com")
        .start()
        .expect("Kurogane failed to initialize");

    let at = |x, y| WindowPlacement {
        x,
        y,
        width: 800,
        height: 600,
        state: WindowState::Normal,
    };

    // Visible immediately
    runtime
        .create_window(
            "https://en.wikipedia.org/wiki/Rust_(programming_language)",
            WindowOptions::new().placement(at(120, 90)),
        )
        .expect("failed to create browser window");

    // Starts maximized; restored, it goes back to its place
    runtime
        .create_window(
            "https://github.com/0x48piraj/kurogane",
            WindowOptions::new()
                .placement(at(240, 180))
                .state(WindowState::Maximized),
        )
        .expect("failed to create browser window");

    // Starts minimized
    runtime
        .create_window(
            "https://www.rust-lang.org",
            WindowOptions::new()
                .placement(at(360, 270))
                .state(WindowState::Minimized),
        )
        .expect("failed to create browser window");

    // Starts hidden, and the application never shows it (a second launch
    // would: it brings every window to the front). Once the other windows
    // close, its browser keeps the application running, as any open browser
    // does; Ctrl+C ends it
    runtime
        .create_window(
            "https://docs.rs",
            WindowOptions::new()
                .placement(at(480, 360))
                .state(WindowState::Hidden),
        )
        .expect("failed to create browser window");

    runtime.run().expect("Kurogane failed to run");
}
