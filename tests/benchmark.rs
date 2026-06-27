use kurogane::{App, sync_json, sync_binary};
use serde_json::Value;

fn main() {
    App::new("benchmark")
        .command("echo", sync_json(|payload: Value| Ok(payload)))
        .command("echo_binary", sync_binary(|data: &[u8]| Ok(data.to_vec())))
        .run_or_exit();
}
