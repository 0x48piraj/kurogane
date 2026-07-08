use kurogane::App;
use kurogane::ipc::{StreamHandler, StreamResponder};

struct EchoStream;

impl StreamHandler for EchoStream {
    fn on_chunk(&mut self, data: &[u8], responder: &StreamResponder) -> Result<(), String> {
        responder.send_data(data)
    }
    fn on_end(&mut self, _result: &str, responder: StreamResponder) -> Result<(), String> {
        responder.end("ok")
    }
}

fn main() {
    App::new("stream-benchmark")
        .stream("echo", || EchoStream)
        .run_or_exit();
}
