use kurogane::{WindowOptions, WindowPlacement, WindowState};

fn main() {
    kurogane_suite::logging();
    let runtime = kurogane::App::url("https://xkcd.com")
        .start()
        .expect("Kurogane failed to initialize");

    // Placed where it is asked, titled after its page
    runtime
        .create_window(
            "https://en.wikipedia.org/wiki/Rust_(programming_language)",
            WindowOptions::new().placement(WindowPlacement {
                x: 120,
                y: 90,
                width: 800,
                height: 600,
                state: WindowState::Normal,
            }),
        )
        .expect("failed to create browser window");

    // Centred at its size, under a title of its own
    runtime
        .create_window(
            "https://github.com/0x48piraj/kurogane",
            WindowOptions::new()
                .title("Kurogane on GitHub")
                .size(1000, 700)
                .min_size(480, 360),
        )
        .expect("failed to create browser window");

    runtime.run().expect("Kurogane failed to run");
}
