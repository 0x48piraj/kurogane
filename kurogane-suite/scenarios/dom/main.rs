use kurogane::App;

fn main() {
    kurogane_suite::logging();
    App::new("scenarios/dom/frontend").run_or_exit();
}
