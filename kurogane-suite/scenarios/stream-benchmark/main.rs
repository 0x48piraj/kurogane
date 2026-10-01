use kurogane::{App, IpcError, StreamHandler, StreamResponder};

struct EchoStream;

impl StreamHandler for EchoStream {
    fn on_chunk(&mut self, data: &[u8], responder: &StreamResponder) -> Result<(), IpcError> {
        responder.send_data(data)
    }
    fn on_end(&mut self, _result: &str, responder: StreamResponder) -> Result<(), IpcError> {
        responder.end("ok")
    }
}

fn main() {
    App::new("scenarios/stream-benchmark/frontend")
        .stream("echo", || EchoStream)
        .run_or_exit();
}
