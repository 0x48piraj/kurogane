use cef::*;
use kurogane::App;

struct BrowserDelegate;

impl kurogane::ClientAppBrowserDelegate for BrowserDelegate {
    fn on_before_command_line_processing(&self, _command_line: &mut CommandLine) {
        println!("[browser delegate] before command-line processing");
    }

    fn on_context_initialized(&self) {
        println!("[browser delegate] browser context initialized");
    }
}

struct RendererDelegate;

impl kurogane::ClientAppRendererDelegate for RendererDelegate {
    fn on_web_kit_initialized(&self) {
        println!("[renderer delegate] web kit initialized");
    }

    fn on_context_created(
        &self,
        _browser: Option<&Browser>,
        frame: Option<&Frame>,
        _context: Option<&V8Context>,
    ) {
        println!("[renderer delegate] JS context created");

        if let Some(frame) = frame {
            frame.execute_java_script(
                Some(&CefString::from(r#"
                    setTimeout(() => {
                        window.core.invoke("ping", { from: "renderer delegate" })
                            .then(result => console.log("[demo] ping result:", result))
                            .catch(err => console.error("[demo] ping failed:", err));
                    }, 1000);
                "#)),
                None,
                0,
            );
        }
    }

    fn on_uncaught_exception(
        &self,
        _browser: Option<&Browser>,
        _frame: Option<&Frame>,
        _context: Option<&V8Context>,
        exception: Option<&V8Exception>,
        _stack_trace: Option<&V8StackTrace>,
    ) {
        if let Some(exception) = exception {
            let msg: cef::CefString = (&exception.message()).into();
            println!("[renderer delegate] uncaught exception: {}", msg.to_string());
        }
    }

    fn on_process_message_received(
        &self,
        _browser: Option<&Browser>,
        _frame: Option<&Frame>,
        source_process: ProcessId,
        _message: Option<&ProcessMessage>,
    ) -> i32 {
        println!("[renderer delegate] process message from {:?}", source_process);
        0
    }
}

fn main() {
    let runtime = App::url("https://example.com")
        .delegate(BrowserDelegate)
        .renderer_delegate(RendererDelegate)
        .command("ping", |payload| {
            Ok(serde_json::json!({
                "ok": true,
                "echo": payload
            }))
        })
        .start()
        .expect("Kurogane failed to initialize");

    while !runtime.should_shutdown() {
        runtime.pump();
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
}
