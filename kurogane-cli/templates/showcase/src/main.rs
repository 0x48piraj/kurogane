#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use kurogane::App;
use serde_json::{Value, json};

fn main() {
    App::new("content")
        .command("echo", |v: Value, _: &kurogane::AppHandle| Ok(v))
        .command("add", |v: Value, _: &kurogane::AppHandle| {
            let a = v["a"].as_i64().unwrap_or(0);
            let b = v["b"].as_i64().unwrap_or(0);
            Ok(json!(a + b))
        })
        .run_or_exit();
}
