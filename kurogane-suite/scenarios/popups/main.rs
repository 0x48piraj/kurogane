fn main() {
    println!("Popups torture test starting...");

    kurogane::App::new("scenarios/popups/frontend")
        .chromium_flag("disable-popup-blocking")
        .run_or_exit();
}
