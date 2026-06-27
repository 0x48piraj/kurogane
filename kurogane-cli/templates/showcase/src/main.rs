use kurogane::{App, sync_json};
use serde_json::{Value, json};

fn main() {
    App::new("content")
        .command("echo", sync_json(|v: Value| Ok(v)))
        .command("add", sync_json(|v: Value| {
            let a = v["a"].as_i64().unwrap_or(0);
            let b = v["b"].as_i64().unwrap_or(0);
            Ok(json!(a + b))
        }))
        .run_or_exit();
}
