use kurogane::{App, sync_json};
use serde_json::{Value, json};

fn main() {
    App::new("content")

        // Simple roundtrip
        .command("ping", sync_json(|_| {
            Ok(json!("pong"))
        }))

        // Greet user
        .command("greet", sync_json(|payload: Value| {
            let name = payload.as_str().unwrap_or("anonymous");
            Ok(json!(format!("Hello, {}!", name)))
        }))

        // Computation with validation
        .command("divide", sync_json(|payload: Value| {
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
        }))

        .run_or_exit();
}
