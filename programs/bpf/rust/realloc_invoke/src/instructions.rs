//! Example Rust-based BPF realloc test program

pub const INVOKE_REALLOC_ZERO_RO: u8 = 0;
pub const INVOKE_REALLOC_ZERO: u8 = 1;
pub const INVOKE_REALLOC_MAX_PLUS_ONE: u8 = 2;
pub const INVOKE_REALLOC_EXTEND_MAX: u8 = 3;
pub const INVOKE_REALLOC_AND_ASSIGN: u8 = 4;
pub const INVOKE_REALLOC_AND_ASSIGN_TO_SELF_VIA_SYSTEM_PROGRAM: u8 = 5;
pub const INVOKE_ASSIGN_TO_SELF_VIA_SYSTEM_PROGRAM_AND_REALLOC: u8 = 6;
pub const INVOKE_REALLOC_INVOKE_CHECK: u8 = 7;
pub const INVOKE_OVERFLOW: u8 = 8;
pub const INVOKE_REALLOC_TO: u8 = 9;
pub const INVOKE_REALLOC_RECURSIVE: u8 = 10;
pub const INVOKE_CREATE_ACCOUNT_REALLOC_CHECK: u8 = 11;
pub const INVOKE_DEALLOC_AND_ASSIGN: u8 = 12;
pub const INVOKE_REALLOC_MAX_TWICE: u8 = 13;
pub const INVOKE_REALLOC_MAX_INVOKE_MAX: u8 = 14;
pub const INVOKE_INVOKE_MAX_TWICE: u8 = 15;
