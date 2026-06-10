use kurogane::App;
use winit::application::ApplicationHandler;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};

struct ViewsDriver {
    handle: kurogane::RuntimeHandle,
}

impl ApplicationHandler for ViewsDriver {
    fn resumed(
        &mut self,
        _: &ActiveEventLoop,
    ) {}

    fn window_event(
        &mut self,
        _: &ActiveEventLoop,
        _: winit::window::WindowId,
        _: winit::event::WindowEvent,
    ) {}

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.handle.pump();

        if self.handle.should_shutdown() {
            event_loop.exit();
        }
    }
}

fn main() {
    let handle = App::url("https://example.com")
        .start()
        .unwrap();

    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = ViewsDriver { handle };
    event_loop.run_app(&mut app).unwrap();
}
