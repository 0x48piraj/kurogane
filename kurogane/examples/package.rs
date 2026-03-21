use kurogane::App;

fn main() {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap().join("content");

    App::path(dir).run_or_exit();
}
