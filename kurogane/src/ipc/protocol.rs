//! IPC protocol definitions and wire format
//!
//! Defines message kinds and helper functions for encoding/decoding
//! ListValue-based messages.

use cef::{ListValue, ImplListValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum IpcMsgKind {
    /// JSON RPC invoke (renderer to browser)
    Invoke = 0,
    /// JSON RPC resolve (browser to renderer)
    Resolve = 1,
    /// JSON RPC reject (browser to renderer)
    Reject = 2,
    /// Binary invoke (renderer to browser)
    BinaryInvoke = 3,
    /// Binary response (browser to renderer)
    BinaryResponse = 4,
    /// SHM free notification (renderer to browser)
    ShmFree = 5,
}

impl IpcMsgKind {
    #[inline]
    pub fn from_int(v: i32) -> Option<Self> {
        match v {
            0 => Some(Self::Invoke),
            1 => Some(Self::Resolve),
            2 => Some(Self::Reject),
            3 => Some(Self::BinaryInvoke),
            4 => Some(Self::BinaryResponse),
            5 => Some(Self::ShmFree),
            _ => None,
        }
    }
}

/// Get message kind from ListValue (index 0)
#[inline]
pub fn get_kind(args: &ListValue) -> Option<IpcMsgKind> {
    IpcMsgKind::from_int(args.int(0))
}

#[inline]
pub fn set_kind(args: &mut ListValue, kind: IpcMsgKind) {
    args.set_int(0, kind as i32);
}
