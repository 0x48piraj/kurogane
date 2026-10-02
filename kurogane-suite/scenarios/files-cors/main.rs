use kurogane::App;

fn main() {
    kurogane_suite::logging();
    App::new("scenarios/files-cors/frontend/dist").run_or_exit();
}
