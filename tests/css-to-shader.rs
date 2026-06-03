use kurogane::App;

fn main() {
    App::new("css-to-shader")
        .chromium_flag_with_value("enable-blink-features", "CanvasDrawElement")
        .run_or_exit();
}
