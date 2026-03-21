use kurogane::App;

fn main() {
    App::url("chrome://gpu")
        .run_or_exit();
}
