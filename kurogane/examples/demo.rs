use kurogane::App;
use serde_json::{Value, json};

fn main() {
    App::new("demo")
        .command("echo", |v: Value| Ok(v))
        .command("add", |v: Value| {
            let a = v["a"].as_i64().unwrap_or(0);
            let b = v["b"].as_i64().unwrap_or(0);
            Ok(json!(a + b))
        })
        .run_or_exit();
}
