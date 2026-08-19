use kurogane::App;
use serde_json::Value;

fn main() {
    App::new("benchmark")
        .command("echo", |payload: Value, _: &kurogane::AppHandle| {
            Ok(payload)
        })
        .binary_command("echo_binary", |data: &[u8], _: &kurogane::AppHandle| {
            Ok(data.to_vec())
        })
        .run_or_exit();
}
