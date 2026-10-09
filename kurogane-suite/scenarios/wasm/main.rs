use kurogane::App;

fn main() {
    kurogane_suite::logging();
    App::new("scenarios/wasm/frontend").run_or_exit();
}
