#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use kurogane::App;
use serde_json::{Value, json};

fn main() {
    App::new("content")

        // Simple roundtrip
        .command("ping", |_: Value, _: &kurogane::AppHandle| {
            Ok(json!("pong"))
        })

        // Greet user
        .command("greet", |payload: Value, _: &kurogane::AppHandle| {
            let name = payload.as_str().unwrap_or("anonymous");
            Ok(json!(format!("Hello, {}!", name)))
        })

        // Computation with validation
        .command("divide", |payload: Value, _: &kurogane::AppHandle| {
            let a = payload["a"]
                .as_f64()
                .ok_or("Missing 'a'")?;

            let b = payload["b"]
                .as_f64()
                .ok_or("Missing 'b'")?;

            if b == 0.0 {
                return Err("Division by zero".into());
            }

            Ok(json!(a / b))
        })

        .run_or_exit();
}
