#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use kurogane::App;

fn main() {
    App::url("http://localhost:8000").run_or_exit();
}
