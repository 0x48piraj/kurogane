use kurogane::App;
use kurogane::tanso::*;

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

    fn on_context_created(&self, _browser: &Browser, frame: &Frame, _context: &V8Context) {
        println!("[renderer delegate] JS context created");

        frame.execute_java_script(
            &CefString::from(
                r#"
                setTimeout(() => {
                    window.core.invoke("ping", { from: "renderer delegate" })
                        .then(result => console.log("[demo] ping result:", result))
                        .catch(err => console.error("[demo] ping failed:", err));
                }, 1000);
            "#,
            ),
            None,
            0,
        );
    }

    fn on_uncaught_exception(
        &self,
        _browser: &Browser,
        _frame: &Frame,
        _context: &V8Context,
        exception: &V8Exception,
        _stack_trace: &V8StackTrace,
    ) {
        let msg: CefString = (&exception.message()).into();
        println!("[renderer delegate] uncaught exception: {}", msg);
    }

    fn on_process_message_received(
        &self,
        _browser: &Browser,
        _frame: &Frame,
        source_process: ProcessId,
        _message: &ProcessMessage,
    ) -> i32 {
        println!(
            "[renderer delegate] process message from {:?}",
            source_process
        );
        0
    }
}

fn main() {
    kurogane_suite::logging();
    let runtime = App::url("https://example.com")
        .delegate(BrowserDelegate)
        .renderer_delegate(RendererDelegate)
        .command(
            "ping",
            |payload: serde_json::Value, _: &kurogane::AppHandle| {
                Ok(serde_json::json!({
                    "ok": true,
                    "echo": payload
                }))
            },
        )
        .start()
        .expect("Kurogane failed to initialize");

    runtime.run().expect("Kurogane failed to run");
}
