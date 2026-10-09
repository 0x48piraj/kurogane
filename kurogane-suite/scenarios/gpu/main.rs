use kurogane::App;

fn main() {
    kurogane_suite::logging();
    App::url("chrome://gpu").run_or_exit();
}
