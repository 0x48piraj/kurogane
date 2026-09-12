use kurogane::App;

fn main() {
    App::new("scenarios/css-to-shader/frontend")
        .chromium_flag_with_value("enable-blink-features", "CanvasDrawElement")
        .run_or_exit();
}
