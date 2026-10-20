//! V8 bridge for IPC
//!
//! Connects JavaScript to the browser process via CEF messages.
//! Defines the boundary between JavaScript and the native IPC system.

use tanso::*;
use std::sync::Arc;
use crate::app::ClientAppRendererDelegate;
use tracing::debug;
use crate::ipc::browser_state::ErrorCode;
use crate::ipc::envelope::*;
use crate::ipc::transport::message::{is_ipc_message, receive_from_browser};
use crate::ipc::renderer_delivery;
use crate::ipc::renderer_outbox;
use crate::ipc::renderer_registry::StreamSink;
use crate::ipc::renderer_state::with_state;
use crate::ipc::utils::rejection;
use crate::ipc::FrameId;
use crate::bridge;

//
// Helpers
//

#[inline(always)]
fn v8_to_string(v: &V8Value) -> String {
    let s: CefString = (&v.string_value()).into();
    s.to_string()
}

/// Sets the V8 exception message and reports the call as handled.
///
/// CEF throws the exception when `Execute` returns `1`. cef-rs exposes the
/// exception as a borrowed `CefString`; `try_set` writes through that wrapper.
fn throw(exception: Option<&mut CefString>, message: &str) -> i32 {
    if let Some(exception) = exception {
        exception.try_set(message);
    }
    1
}

/// Returns `value` as a byte offset or length when it is a whole number from
/// zero up.
fn byte_index(value: &V8Value) -> Option<usize> {
    if value.is_int() == 0 && value.is_uint() == 0 && value.is_double() == 0 {
        return None;
    }
    let index = value.double_value();
    (index >= 0.0 && index.fract() == 0.0).then_some(index as usize)
}

/// Returns the offset and length that follow an ArrayBuffer argument, or
/// `None` for the whole buffer when nothing follows it. `Err` when the
/// arguments are not two whole numbers.
fn parse_view(rest: &[Option<V8Value>]) -> Result<Option<(usize, usize)>, ()> {
    match rest {
        [] => Ok(None),
        [Some(offset), Some(len)] => match (byte_index(offset), byte_index(len)) {
            (Some(offset), Some(len)) => Ok(Some((offset, len))),
            _ => Err(()),
        },
        _ => Err(()),
    }
}

/// Returns how many bytes of `buffer` a message sends, those `view` names or
/// the whole buffer's.
fn data_len(buffer: &V8Value, view: Option<(usize, usize)>) -> usize {
    view.map_or_else(|| buffer.array_buffer_byte_length(), |(_, len)| len)
}

/// Why a page's call or stream open was not sent.
enum Unsent {
    /// Its data exceeds [`MESSAGE_MAX`].
    TooLarge,
    /// The message could not be built.
    Unbuilt,
}

impl Unsent {
    /// Returns the rejection the page's promise gets.
    fn rejection(&self) -> CefString {
        match self {
            Self::TooLarge => rejection(ErrorCode::TooLarge.wire(), MESSAGE_TOO_LARGE),
            Self::Unbuilt => rejection(ErrorCode::Buffer.wire(), "Failed to build IPC message"),
        }
    }
}

/// Sends a message of `frame`'s page whose payload is `header` then the
/// bytes of `buffer` that `view` names as an offset and length, or every
/// byte without a view. `None` when a non-empty buffer has no memory, the
/// view lies outside the buffer or the message cannot be built.
///
/// The only reader of an ArrayBuffer's memory. The outbox only copies the
/// bytes. No JavaScript runs while they are borrowed (a script could detach
/// or resize the buffer). Any page script can pass a view. The view is
/// checked against the buffer's length.
fn send_buffer(
    frame: &Frame,
    frame_id: &FrameId,
    envelope: &Envelope,
    header: &[u8],
    buffer: &V8Value,
    view: Option<(usize, usize)>,
) -> Option<()> {
    let len = buffer.array_buffer_byte_length();
    let whole: &[u8] = if len == 0 {
        &[]
    } else {
        let data = buffer.array_buffer_data() as *const u8;
        if data.is_null() {
            return None;
        }
        // SAFETY: V8 keeps an ArrayBuffer's `len` bytes at `data` until
        // JavaScript detaches or resizes it, and `buffer` keeps it alive. No
        // JavaScript runs before the slice's last use, since the outbox only
        // copies it
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    let bytes = match view {
        Some((offset, len)) => whole.get(offset..)?.get(..len)?,
        None => whole,
    };
    let parts = [header, bytes];
    let parts = if header.is_empty() {
        &parts[1..]
    } else {
        &parts[..]
    };
    renderer_outbox::send(frame, frame_id, envelope, parts)
}

/// Encode [cmd_len:u16 LE][cmd_bytes] into a Vec.
///
/// Returns None when the command exceeds [`MAX_CMD_LEN`]; see
/// [`encode_cmd_payload`] for why this is not an `as` cast.
fn encode_cmd_header(cmd: &str) -> Option<Vec<u8>> {
    let cmd_bytes = cmd.as_bytes();
    if cmd_bytes.len() > MAX_CMD_LEN {
        return None;
    }

    let mut v = Vec::with_capacity(2 + cmd_bytes.len());
    // Narrowing is safe; bounded by MAX_CMD_LEN on the line above
    let cmd_len = cmd_bytes.len() as u16;
    v.extend_from_slice(&cmd_len.to_le_bytes());
    v.extend_from_slice(cmd_bytes);
    Some(v)
}

//
// Renderer process handler
//

wrap_render_process_handler! {
    pub struct IpcRenderProcessHandler {
        delegates: Vec<Arc<dyn ClientAppRendererDelegate>>,
    }

    impl RenderProcessHandler {
        fn on_web_kit_initialized(&self) {
            for delegate in &self.delegates {
                delegate.on_web_kit_initialized();
            }
        }

        fn on_browser_created(
            &self,
            browser: Option<&mut Browser>,
            extra_info: Option<&mut DictionaryValue>,
        ) {
            let browser_ref = browser.as_deref();
            let extra_info_ref = extra_info.as_deref();

            for delegate in &self.delegates {
                delegate.on_browser_created(browser_ref, extra_info_ref);
            }
        }

        fn on_browser_destroyed(
            &self,
            browser: Option<&mut Browser>,
        ) {
            let browser_ref = browser.as_deref();

            for delegate in &self.delegates {
                delegate.on_browser_destroyed(browser_ref);
            }
        }

        fn on_context_created(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            context: Option<&mut V8Context>,
        ) {
            let (Some(context), Some(frame)) = (context, frame) else {
                return;
            };
            let Some(global) = context.global() else {
                return;
            };
            let Some(mut core) = v8_value_create_object(None, None) else {
                return;
            };

            // A bridge missing a function is not installed at all
            let functions = [
                // Accepts a string or ArrayBuffer payload
                ("invoke", IpcInvokeHandler::new()),
                // Cancels a pending promise
                ("cancel", IpcCancelHandler::new()),
                ("on", IpcOnHandler::new()),
                ("off", IpcOffHandler::new()),
                ("openStream", IpcOpenStreamHandler::new()),
                ("writeStream", IpcWriteStreamHandler::new()),
                ("endStream", IpcEndStreamHandler::new()),
                // Unpaces the page's messages while it leaves
                ("leaving", IpcLeavingHandler::new()),
                // Settles what a page back from the back-forward cache left
                // open and subscribes it again
                ("returned", IpcReturnedHandler::new()),
            ];
            for (name, handler) in functions {
                if !set_function(&core, name, handler) {
                    return;
                }
            }

            // Read before any page script runs: the document's own view of
            // its origin. A sandboxed document's (iframe `sandbox`, CSP
            // `sandbox`) is "null", whatever its URL; its messages then act
            // for the opaque origin. Unreadable counts as opaque.
            let opaque = !matches!(
                global.value_bykey(Some(&CefString::from("origin"))),
                Some(origin) if origin.is_string() != 0 && v8_to_string(&origin) != "null"
            );
            with_state(|state| state.context_created(context.clone(), FrameId::of(frame), opaque));
            // The browser sees only URLs, which hide a sandbox: it learns of
            // the document here, for what it reports of a frame
            // (crate::context_menu's target)
            if opaque {
                tell_browser(frame, crate::context_menu::OPAQUE_DOCUMENT);
            }

            global.set_value_bykey(
                Some(&CefString::from("core")),
                Some(&mut core),
                V8Propertyattribute::default(),
            );

            frame.execute_java_script(
                Some(&CefString::from(bridge::KUROGANE_BRIDGE)),
                None,
                0,
            );

            debug!("[IPC Renderer] Injected window.core.* + kurogane bridge");

            let browser_ref = browser.as_deref();
            for delegate in &self.delegates {
                delegate.on_context_created(browser_ref, Some(frame), Some(context));
            }
        }

        fn on_context_released(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            context: Option<&mut V8Context>,
        ) {
            let context_ref = context.as_deref();
            let browser_ref = browser.as_deref();
            let frame_ref = frame.as_deref();

            for delegate in &self.delegates {
                delegate.on_context_released(browser_ref, frame_ref, context_ref);
            }

            // Messages the page sent just before its context went still leave
            if let Some(frame) = frame_ref {
                renderer_outbox::release_frame(&FrameId::of(frame));
            }

            if let Some(ctx) = context {
                let opaque = with_state(|state| state.is_opaque(ctx));
                with_state(|state| state.context_released(ctx));
                if opaque && let Some(frame) = frame {
                    tell_browser(frame, crate::context_menu::OPAQUE_DOCUMENT_GONE);
                }
            }
        }

        fn on_uncaught_exception(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            context: Option<&mut V8Context>,
            exception: Option<&mut V8Exception>,
            stack_trace: Option<&mut V8StackTrace>,
        ) {
            let browser_ref = browser.as_deref();
            let frame_ref = frame.as_deref();
            let context_ref = context.as_deref();
            let exception_ref = exception.as_deref();
            let stack_trace_ref = stack_trace.as_deref();

            for delegate in &self.delegates {
                delegate.on_uncaught_exception(
                    browser_ref, frame_ref, context_ref, exception_ref, stack_trace_ref,
                );
            }

            if let Some(ex) = exception {
                let msg: CefString = (&ex.message()).into();
                let src: CefString = (&ex.script_resource_name()).into();
                let line = ex.line_number();
                debug!("[Renderer] Uncaught exception at {}:{} - {}", src.to_string(), line, msg.to_string());
            }
        }

        fn on_focused_node_changed(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            node: Option<&mut Domnode>,
        ) {
            let Some(node_data) = node.as_ref() else {
                let browser_ref = browser.as_deref();
                let frame_ref = frame.as_deref();

                for delegate in &self.delegates {
                    delegate.on_focused_node_changed(browser_ref, frame_ref, None);
                }
                return;
            };
            let is_editable = node_data.is_editable() != 0;
            let is_form = node_data.is_form_control_element() != 0;

            debug!(
                "[Renderer] Focused node changed: type={} editable={} form={} form_type={}",
                node_data.get_type().get_raw(),
                is_editable,
                is_form,
                is_form
                    .then(|| format!("{:?}", node_data.form_control_element_type()))
                    .as_deref()
                    .unwrap_or("-"),
            );

            let browser_ref = browser.as_deref();
            let frame_ref = frame.as_deref();
            let node_ref = Some(&**node_data);

            for delegate in &self.delegates {
                delegate.on_focused_node_changed(browser_ref, frame_ref, node_ref);
            }
        }

        fn on_process_message_received(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            source_process: ProcessId,
            message: Option<&mut ProcessMessage>,
        ) -> i32 {
            let ipc = source_process == ProcessId::BROWSER
                && message.as_deref().is_some_and(is_ipc_message);

            // Kurogane's own messages never reach the delegates. One a
            // delegate kept would hold its frame's later messages for good
            if ipc {
                let (Some(frame), Some(msg)) = (frame, message) else {
                    debug!("[IPC Renderer] missing frame");
                    return 0;
                };

                // SAFETY: the browser sent it, as source_process says
                let Some(received) = (unsafe { receive_from_browser(msg) }) else {
                    debug!("[IPC Renderer] failed to extract message");
                    return 1;
                };

                renderer_delivery::receive(frame, received);
                return 1;
            }

            let browser_ref = browser.as_deref();
            let frame_ref = frame.as_deref();
            let message_ref = message.as_deref();
            for delegate in &self.delegates {
                if delegate.on_process_message_received(
                    browser_ref, frame_ref, source_process, message_ref,
                ) != 0 {
                    return 1;
                }
            }
            0
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            for delegate in &self.delegates {
                if let Some(handler) = delegate.load_handler() {
                    return Some(handler);
                }
            }
            None
        }
    }
}

//
// Invoke handler (accepts string or ArrayBuffer payload)
//

/// Sets `handler` on `object` as the function `name`. Returns false when V8
/// cannot create the function.
fn set_function(object: &V8Value, name: &str, mut handler: V8Handler) -> bool {
    let name = CefString::from(name);
    let Some(mut function) = v8_value_create_function(Some(&name), Some(&mut handler)) else {
        return false;
    };
    object.set_value_bykey(
        Some(&name),
        Some(&mut function),
        V8Propertyattribute::default(),
    );
    true
}

wrap_v8_handler! {
    pub struct IpcInvokeHandler;

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            arguments: Option<&[Option<V8Value>]>,
            retval: Option<&mut Option<V8Value>>,
            exception: Option<&mut CefString>,
        ) -> i32 {
            // args must be present
            let args = match arguments {
                Some(a) if !a.is_empty() => a,
                _ => {
                    return throw(exception, "invoke requires at least a command argument");
                }
            };

            // first arg: command string
            let cmd = match args.first() {
                Some(Some(v)) if v.is_string() != 0 => {
                    let s = v8_to_string(v);
                    if s.is_empty() {
                        return throw(exception, "command cannot be empty");
                    }
                    s
                }
                _ => {
                    return throw(exception, "command must be a non-empty string");
                }
            };

            let context = match v8_context_get_current_context() {
                Some(ctx) => ctx,
                None => {
                    return throw(exception, "invoke: no active renderer context");
                }
            };

            let Some(frame) = context.frame() else {
                return throw(exception, "invoke: no frame for current context");
            };

            // Detect payload type from second argument
            let binary = args.get(1)
                .and_then(|v| v.as_ref())
                .filter(|v| v.is_array_buffer() != 0);
            let Ok(view) = binary.map_or(Ok(None), |_| parse_view(args.get(2..).unwrap_or_default())) else {
                return throw(exception, "invoke: byteOffset and byteLength must be whole numbers");
            };

            let Some(cmd_header) = encode_cmd_header(&cmd) else {
                return throw(
                    exception,
                    "invoke: command name exceeds the 65535-byte protocol limit",
                );
            };

            let Some(promise) = v8_value_create_promise() else {
                return throw(exception, "invoke: cannot create a promise");
            };
            let promise_for_retval = promise.clone();
            let registered = with_state(|state| state.register_rpc(&context, promise.clone()));
            let Some((id, flags, frame_id)) = registered else {
                return throw(exception, "invoke: this context is not connected");
            };

            // Expose the id on the promise for JS-side cancellation
            let id_key = CefString::from("__kurogane_id");
            let Some(mut id_value) = v8_value_create_int(id) else {
                with_state(|state| state.forget(id));
                return throw(exception, "invoke: cannot create the call id");
            };
            promise_for_retval.set_value_bykey(
                Some(&id_key),
                Some(&mut id_value),
                V8Propertyattribute::default(),
            );

            debug!("[IPC Renderer] JS invoke: '{}' (id={}, binary={})", cmd, id, binary.is_some());

            let sent = if let Some(buffer) = binary {
                let envelope = Envelope {
                    version: ENVELOPE_VERSION,
                    subsystem: SUB_RPC,
                    opcode: RPC_INVOKE,
                    flags,
                    correlation_id: id as u32,
                    payload_kind: PAYLOAD_BINARY,
                };

                if data_len(buffer, view) > MESSAGE_MAX {
                    Err(Unsent::TooLarge)
                } else {
                    send_buffer(&frame, &frame_id, &envelope, &cmd_header, buffer, view)
                        .ok_or(Unsent::Unbuilt)
                }
            } else {
                let payload = match args.get(1) {
                    Some(Some(v)) if v.is_string() != 0 => v8_to_string(v),
                    _ => String::new(),
                };

                let envelope = Envelope {
                    version: ENVELOPE_VERSION,
                    subsystem: SUB_RPC,
                    opcode: RPC_INVOKE,
                    flags,
                    correlation_id: id as u32,
                    payload_kind: PAYLOAD_STRING,
                };

                if payload.len() > MESSAGE_MAX {
                    Err(Unsent::TooLarge)
                } else {
                    renderer_outbox::send(&frame, &frame_id, &envelope, &[&cmd_header, payload.as_bytes()])
                        .ok_or(Unsent::Unbuilt)
                }
            };

            if let Err(unsent) = sent {
                with_state(|state| state.forget(id));
                promise.reject_promise(Some(&unsent.rejection()));
                if let Some(ret) = retval { *ret = Some(promise_for_retval); }
                return 1;
            }

            if let Some(ret) = retval {
                *ret = Some(promise_for_retval);
            }

            1
        }
    }
}

//
// Cancel handler
//

wrap_v8_handler! {
    pub struct IpcCancelHandler;

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            arguments: Option<&[Option<V8Value>]>,
            retval: Option<&mut Option<V8Value>>,
            exception: Option<&mut CefString>,
        ) -> i32 {
            let args = match arguments {
                Some(a) if !a.is_empty() => a,
                _ => {
                    return throw(exception, "cancel requires an id argument");
                }
            };

            let id = match args.first() {
                Some(Some(v)) if v.is_int() != 0 || v.is_uint() != 0 => v.int_value(),
                _ => {
                    return throw(exception, "cancel: id must be an integer");
                }
            };

            let Some(context) = v8_context_get_current_context() else {
                if let Some(ret) = retval { *ret = v8_value_create_bool(0); }
                return 1;
            };
            // Only the context that made a request can cancel it
            let (cancelled, sender) =
                with_state(|state| (state.cancel(id, &context), state.sender(&context)));
            let (Some((promise, sub)), Some((flags, frame_id))) = (cancelled, sender) else {
                if let Some(ret) = retval { *ret = v8_value_create_bool(0); }
                return 1;
            };

            let opcode = match sub {
                SUB_STREAM => STREAM_CANCEL,
                _ => RPC_CANCEL,
            };
            let envelope = Envelope {
                version: ENVELOPE_VERSION,
                subsystem: sub,
                opcode,
                flags,
                correlation_id: id as u32,
                payload_kind: PAYLOAD_EMPTY,
            };
            if let Some(frame) = context.frame() {
                // A canceled call may get no reply
                if sub == SUB_RPC {
                    renderer_outbox::answered(&frame_id, id as u32);
                }
                renderer_outbox::send(&frame, &frame_id, &envelope, &[]);
            }

            // The page calls from the owner's context, which is entered
            // already. Entering it again here, inside a checkpoint its own
            // exit runs, would stop the page's microtasks
            let reject_msg = rejection(ErrorCode::Handler.wire(), "Canceled");
            promise.reject_promise(Some(&reject_msg));
            if let Some(ret) = retval {
                *ret = v8_value_create_bool(1);
            }

            1
        }
    }
}

//
// Leaving handler
//

wrap_v8_handler! {
    pub struct IpcLeavingHandler;

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            _arguments: Option<&[Option<V8Value>]>,
            _retval: Option<&mut Option<V8Value>>,
            _exception: Option<&mut CefString>,
        ) -> i32 {
            let Some(context) = v8_context_get_current_context() else {
                return 1;
            };
            let sender = with_state(|state| state.sender(&context));
            if let (Some((_, frame_id)), Some(frame)) = (sender, context.frame()) {
                renderer_outbox::leaving(&frame, &frame_id, true);
            }
            1
        }
    }
}

//
// Returned handler
//

wrap_v8_handler! {
    pub struct IpcReturnedHandler;

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            _arguments: Option<&[Option<V8Value>]>,
            _retval: Option<&mut Option<V8Value>>,
            _exception: Option<&mut CefString>,
        ) -> i32 {
            let Some(context) = v8_context_get_current_context() else {
                return 1;
            };
            let (returned, sender) =
                with_state(|state| (state.returned(&context), state.sender(&context)));
            let (Some((flags, frame_id)), Some(frame)) = (sender, context.frame()) else {
                return 1;
            };
            let send = |subsystem, opcode, id: i32, parts: &[&[u8]]| {
                let envelope = Envelope {
                    version: ENVELOPE_VERSION,
                    subsystem,
                    opcode,
                    flags,
                    correlation_id: id as u32,
                    payload_kind: PAYLOAD_EMPTY,
                };
                renderer_outbox::send(&frame, &frame_id, &envelope, parts);
            };

            // The browser ended the page's requests and streams as it left;
            // each is cancelled again in case its document lived on
            let left = rejection(ErrorCode::Left.wire(), "The page left");
            for (id, subsystem, promise) in returned.requests {
                if subsystem == SUB_RPC {
                    renderer_outbox::answered(&frame_id, id as u32);
                    send(SUB_RPC, RPC_CANCEL, id, &[]);
                } else {
                    send(SUB_STREAM, STREAM_CANCEL, id, &[]);
                }
                promise.reject_promise(Some(&left));
            }
            for (id, on_error) in returned.streams {
                send(SUB_STREAM, STREAM_CANCEL, id, &[]);
                let message = v8_value_create_string(Some(&CefString::from("The page left")));
                on_error.execute_function(None, Some(&[message]));
            }
            // A subscription the browser kept takes the same id again
            for (id, event) in returned.subscriptions {
                if let Some(payload) = encode_cmd_payload(&event, &[]) {
                    send(SUB_EVENT, EVENT_SUBSCRIBE, id, &[&payload]);
                }
            }
            renderer_outbox::leaving(&frame, &frame_id, false);
            1
        }
    }
}

//
// Event on handler
//

wrap_v8_handler! {
    pub struct IpcOnHandler;

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            arguments: Option<&[Option<V8Value>]>,
            retval: Option<&mut Option<V8Value>>,
            exception: Option<&mut CefString>,
        ) -> i32 {
            let args = match arguments {
                Some(a) if a.len() >= 2 => a,
                _ => {
                    return throw(
                        exception,
                        "on(eventName, callback[, onError]) requires two arguments",
                    );
                }
            };

            let event_name = match args.first() {
                Some(Some(v)) if v.is_string() != 0 => v8_to_string(v),
                _ => {
                    return throw(exception, "event name must be a string");
                }
            };

            if event_name.is_empty() {
                return throw(exception, "event name cannot be empty");
            }

            let callback = match args.get(1) {
                Some(Some(v)) if v.is_function() != 0 => v,
                _ => {
                    return throw(exception, "second argument must be a function");
                }
            };

            // Optional callback invoked when the subscription is refused
            let on_error = match args.get(2) {
                Some(Some(v)) if v.is_function() != 0 => Some(v.clone()),
                Some(Some(v)) if v.is_undefined() == 0 && v.is_null() == 0 => {
                    return throw(exception, "third argument must be a function");
                }
                _ => None,
            };

            let context = match v8_context_get_current_context() {
                Some(ctx) => ctx,
                None => {
                    return throw(exception, "on: no active renderer context");
                }
            };

            let Some(frame) = context.frame() else {
                return throw(exception, "on: no frame for current context");
            };

            let Some(payload) = encode_cmd_payload(&event_name, &[]) else {
                return throw(exception, "on: event name exceeds the 65535-byte protocol limit");
            };

            let (id, sender) = with_state(|state| {
                let id = state.subscribe(&context, event_name.clone(), callback.clone(), on_error);
                (id, state.sender(&context))
            });
            let (Some(id), Some((flags, frame_id))) = (id, sender) else {
                return throw(exception, "on: this context is not connected");
            };

            let envelope = Envelope {
                version: ENVELOPE_VERSION,
                subsystem: SUB_EVENT,
                opcode: EVENT_SUBSCRIBE,
                flags,
                correlation_id: id as u32,
                payload_kind: PAYLOAD_EMPTY,
            };
            if renderer_outbox::send(&frame, &frame_id, &envelope, &[&payload]).is_none() {
                debug!("[IPC Renderer] failed to build subscribe message");
            }

            debug!("[IPC Renderer] event on '{}' id={}", event_name, id);

            if let Some(ret) = retval {
                *ret = v8_value_create_uint(id as u32);
            }

            1
        }
    }
}

//
// Event off handler
//

wrap_v8_handler! {
    pub struct IpcOffHandler;

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            arguments: Option<&[Option<V8Value>]>,
            retval: Option<&mut Option<V8Value>>,
            exception: Option<&mut CefString>,
        ) -> i32 {
            let args = match arguments {
                Some(a) if !a.is_empty() => a,
                _ => {
                    return throw(exception, "off requires an id argument");
                }
            };

            let id = match args.first() {
                Some(Some(v)) if v.is_int() != 0 || v.is_uint() != 0 => v.int_value(),
                _ => {
                    return throw(exception, "off: id must be an integer");
                }
            };

            let context = match v8_context_get_current_context() {
                Some(ctx) => ctx,
                None => {
                    return throw(exception, "off: no active renderer context");
                }
            };

            // Only the context that subscribed can unsubscribe
            let (event_name, sender) =
                with_state(|state| (state.unsubscribe(id, &context), state.sender(&context)));
            let was_valid = event_name.is_some();

            if let Some(event_name) = event_name
                && let Some((flags, frame_id)) = sender
                && let Some(frame) = context.frame()
                && let Some(payload) = encode_cmd_payload(&event_name, &[])
            {
                let envelope = Envelope {
                    version: ENVELOPE_VERSION,
                    subsystem: SUB_EVENT,
                    opcode: EVENT_UNSUBSCRIBE,
                    flags,
                    correlation_id: id as u32,
                    payload_kind: PAYLOAD_EMPTY,
                };
                renderer_outbox::send(&frame, &frame_id, &envelope, &[&payload]);
            }

            if let Some(ret) = retval {
                *ret = v8_value_create_bool(if was_valid { 1 } else { 0 });
            }
            1
        }
    }
}

//
// Stream open handler
//

wrap_v8_handler! {
    pub struct IpcOpenStreamHandler;

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            arguments: Option<&[Option<V8Value>]>,
            retval: Option<&mut Option<V8Value>>,
            exception: Option<&mut CefString>,
        ) -> i32 {
            let args = match arguments {
                Some(a) if !a.is_empty() => a,
                _ => {
                    return throw(exception, "openStream requires a handler name argument");
                }
            };

            let handler_name = match args.first() {
                Some(Some(v)) if v.is_string() != 0 => v8_to_string(v),
                _ => {
                    return throw(exception, "handler name must be a string");
                }
            };

            if handler_name.is_empty() {
                return throw(exception, "handler name cannot be empty");
            }

            let metadata = match args.get(1) {
                Some(Some(v)) if v.is_string() != 0 => v8_to_string(v),
                _ => String::new(),
            };

            // The stream's callbacks are bound here, atomically with the open,
            // so no other frame can ever attach to this stream
            let function = |i: usize| match args.get(i) {
                Some(Some(v)) if v.is_function() != 0 => Some(v.clone()),
                _ => None,
            };
            let (Some(data), Some(end), Some(error)) = (function(2), function(3), function(4)) else {
                return throw(
                    exception,
                    "openStream(name, metadata, onData, onEnd, onError) needs three callbacks",
                );
            };

            let context = match v8_context_get_current_context() {
                Some(ctx) => ctx,
                None => {
                    return throw(exception, "openStream: no active renderer context");
                }
            };

            let Some(frame) = context.frame() else {
                return throw(exception, "openStream: no frame for current context");
            };

            let Some(promise) = v8_value_create_promise() else {
                return throw(exception, "openStream: cannot create a promise");
            };
            let sink = StreamSink { data, end, error };
            let (stream_id, sender) = with_state(|state| {
                (state.register_stream_open(&context, promise.clone(), sink), state.sender(&context))
            });
            let (Some(stream_id), Some((flags, frame_id))) = (stream_id, sender) else {
                return throw(exception, "openStream: this context is not connected");
            };

            debug!("[IPC Renderer] openStream '{}' stream_id={}", handler_name, stream_id);

            let envelope = Envelope {
                version: ENVELOPE_VERSION,
                subsystem: SUB_STREAM,
                opcode: STREAM_OPEN,
                flags,
                correlation_id: stream_id as u32,
                payload_kind: PAYLOAD_STRING,
            };

            let sent = if metadata.len() > MESSAGE_MAX {
                Err(Unsent::TooLarge)
            } else {
                encode_cmd_payload(&handler_name, metadata.as_bytes())
                    .and_then(|payload| renderer_outbox::send(&frame, &frame_id, &envelope, &[&payload]))
                    .ok_or(Unsent::Unbuilt)
            };
            if let Err(unsent) = sent {
                with_state(|state| state.forget(stream_id));
                promise.reject_promise(Some(&unsent.rejection()));
            }

            // The promise settles when the browser answers the open:
            // STREAM_BROWSER_OPENED resolves it, STREAM_BROWSER_ERROR rejects it

            if let Some(ret) = retval {
                *ret = Some(promise);
            }

            1
        }
    }
}

//
// Stream write handler
//

wrap_v8_handler! {
    pub struct IpcWriteStreamHandler;

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            arguments: Option<&[Option<V8Value>]>,
            retval: Option<&mut Option<V8Value>>,
            exception: Option<&mut CefString>,
        ) -> i32 {
            let args = match arguments {
                Some(a) if a.len() >= 2 => a,
                _ => {
                    return throw(exception, "writeStream(streamId, ArrayBuffer)");
                }
            };

            let stream_id = match args.first() {
                Some(Some(v)) if v.is_int() != 0 || v.is_uint() != 0 => v.int_value(),
                _ => {
                    return throw(exception, "writeStream: streamId must be an integer");
                }
            };

            let buffer = match args.get(1) {
                Some(Some(v)) if v.is_array_buffer() != 0 => v,
                _ => {
                    return throw(exception, "writeStream: second argument must be an ArrayBuffer");
                }
            };
            let Ok(view) = parse_view(args.get(2..).unwrap_or_default()) else {
                return throw(exception, "writeStream: byteOffset and byteLength must be whole numbers");
            };
            if data_len(buffer, view) > MESSAGE_MAX {
                return throw(exception, "writeStream: a chunk carries at most 64 MiB; write smaller chunks");
            }

            let context = match v8_context_get_current_context() {
                Some(ctx) => ctx,
                None => {
                    return throw(exception, "writeStream: no active renderer context");
                }
            };

            let Some(frame) = context.frame() else {
                return throw(exception, "writeStream: no frame for current context");
            };

            let sender = with_state(|state| {
                state.owns_stream(stream_id, &context).then(|| state.sender(&context)).flatten()
            });
            let Some((flags, frame_id)) = sender else {
                if let Some(ret) = retval { *ret = v8_value_create_bool(0); }
                return 1;
            };

            let envelope = Envelope {
                version: ENVELOPE_VERSION,
                subsystem: SUB_STREAM,
                opcode: STREAM_DATA,
                flags,
                correlation_id: stream_id as u32,
                payload_kind: PAYLOAD_BINARY,
            };

            send_buffer(&frame, &frame_id, &envelope, &[], buffer, view);

            if let Some(ret) = retval {
                *ret = v8_value_create_uint(1);
            }

            1
        }
    }
}

//
// Stream end handler
//

wrap_v8_handler! {
    pub struct IpcEndStreamHandler;

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            arguments: Option<&[Option<V8Value>]>,
            retval: Option<&mut Option<V8Value>>,
            exception: Option<&mut CefString>,
        ) -> i32 {
            let args = match arguments {
                Some(a) if !a.is_empty() => a,
                _ => {
                    return throw(exception, "endStream requires a streamId argument");
                }
            };

            let stream_id = match args.first() {
                Some(Some(v)) if v.is_int() != 0 || v.is_uint() != 0 => v.int_value(),
                _ => {
                    return throw(exception, "endStream: streamId must be an integer");
                }
            };

            let result = match args.get(1) {
                Some(Some(v)) if v.is_string() != 0 => v8_to_string(v),
                _ => String::new(),
            };
            if result.len() > MESSAGE_MAX {
                return throw(exception, "endStream: a result carries at most 64 MiB");
            }

            let context = match v8_context_get_current_context() {
                Some(ctx) => ctx,
                None => {
                    return throw(exception, "endStream: no active renderer context");
                }
            };

            let Some(frame) = context.frame() else {
                return throw(exception, "endStream: no frame for current context");
            };

            let sender = with_state(|state| {
                state.owns_stream(stream_id, &context).then(|| state.sender(&context)).flatten()
            });
            let Some((flags, frame_id)) = sender else {
                if let Some(ret) = retval { *ret = v8_value_create_bool(0); }
                return 1;
            };

            let payload = result.as_bytes();

            let envelope = Envelope {
                version: ENVELOPE_VERSION,
                subsystem: SUB_STREAM,
                opcode: STREAM_END,
                flags,
                correlation_id: stream_id as u32,
                payload_kind: PAYLOAD_STRING,
            };

            renderer_outbox::send(&frame, &frame_id, &envelope, &[payload]);

            if let Some(ret) = retval {
                *ret = v8_value_create_uint(1);
            }

            1
        }
    }
}

/// Sends the browser a message without a body, named `name`, after the
/// page's messages.
fn tell_browser(frame: &Frame, name: &str) {
    if let Some(mut message) = process_message_create(Some(&CefString::from(name))) {
        renderer_outbox::send_after(frame, &FrameId::of(frame), &mut message);
    }
}
