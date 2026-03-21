use kurogane::App;
use serde_json::{Value};

fn main() {
    App::path("benchmark")
        .command("echo", |payload: Value| {
            println!("[echo] {:?}", payload);
            Ok(payload)
        })
        .binary_command("echo_binary", |data: &[u8]| {
            Ok(data.to_vec())
        })
        .run_or_exit();
}
