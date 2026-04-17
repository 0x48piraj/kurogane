use cef::{ListValue, ImplListValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum IpcMsgKind {
    Invoke = 0,
    Resolve = 1,
    Reject = 2,
    BinaryInvoke = 3,
    BinaryResponse = 4,
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

    #[inline]
    pub fn as_int(self) -> i32 {
        self as i32
    }
}

#[inline]
pub fn get_kind(args: &ListValue) -> Option<IpcMsgKind> {
    IpcMsgKind::from_int(args.int(0))
}

#[inline]
pub fn set_kind(args: &mut ListValue, kind: IpcMsgKind) {
    args.set_int(0, kind.as_int());
}
