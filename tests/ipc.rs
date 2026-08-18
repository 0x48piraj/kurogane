use kurogane::App;
use kurogane::IpcError;
use serde_json::{Value, json};

fn main() {
    App::new("ipc")
        // Echo: returns exactly what was sent
        .command("echo", |payload: Value, _: &kurogane::AppHandle| {
            println!("[echo] {:?}", payload);
            Ok(payload)
        })
        // Greeting: expects a string, returns a string
        .command("greet", |payload: Value, _: &kurogane::AppHandle| {
            let name = payload.as_str().unwrap_or("anonymous");
            println!("[greet] {}", name);

            Ok(json!(format!("Hello, {}!", name)))
        })
        // Divide: expects { a: number, b: number }
        .command("divide", |payload: Value, _: &kurogane::AppHandle| {
            println!("[divide] {:?}", payload);

            let a = payload["a"]
                .as_f64()
                .ok_or_else(|| IpcError::new("Missing or invalid 'a'", 0))?;

            let b = payload["b"]
                .as_f64()
                .ok_or_else(|| IpcError::new("Missing or invalid 'b'", 0))?;

            if b == 0.0 {
                return Err(IpcError::new("Division by zero", 0));
            }

            Ok(json!(a / b))
        })
        // File system mock
        .command("fs.read", |payload: Value, _: &kurogane::AppHandle| {
            let file = payload.as_str().unwrap_or("");
            println!("[fs.read] {}", file);

            match file {
                "config.json" => Ok(json!({
                    "name": "MyApp",
                    "version": "1.0.0"
                })),
                "data.txt" => Ok(json!("Sample file contents")),
                _ => Err(IpcError::new(format!("File not found: {}", file), 0)),
            }
        })
        // Slow operation: demonstrates blocking behavior
        .command(
            "slow_operation",
            |payload: Value, _: &kurogane::AppHandle| {
                println!("[slow_operation] starting...");

                std::thread::sleep(std::time::Duration::from_millis(500));

                Ok(json!({
                    "status": "done",
                    "input": payload
                }))
            },
        )
        // Type inspector: proves structured JSON transport
        .command("types", |payload: Value, _: &kurogane::AppHandle| {
            Ok(json!({
                "is_object": payload.is_object(),
                "is_array": payload.is_array(),
                "is_string": payload.is_string(),
                "is_number": payload.is_number(),
                "is_bool": payload.is_boolean(),
                "is_null": payload.is_null(),
            }))
        })
        .binary_command("echo_binary", |data: &[u8], _: &kurogane::AppHandle| {
            Ok(data.to_vec())
        })
        .run_or_exit();
}
