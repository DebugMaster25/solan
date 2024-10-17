//! Example Rust-based SBF program that issues a cross-program-invocation

pub const TEST_SUCCESS: u8 = 1;
pub const TEST_PRIVILEGE_ESCALATION_SIGNER: u8 = 2;
pub const TEST_PRIVILEGE_ESCALATION_WRITABLE: u8 = 3;
pub const TEST_PPROGRAM_NOT_OWNED_BY_LOADER: u8 = 4;
pub const TEST_PPROGRAM_NOT_EXECUTABLE: u8 = 5;
pub const TEST_EMPTY_ACCOUNTS_SLICE: u8 = 6;
pub const TEST_CAP_SEEDS: u8 = 7;
pub const TEST_CAP_SIGNERS: u8 = 8;
pub const TEST_ALLOC_ACCESS_VIOLATION: u8 = 9;
pub const TEST_MAX_INSTRUCTION_DATA_LEN_EXCEEDED: u8 = 10;
pub const TEST_MAX_INSTRUCTION_ACCOUNTS_EXCEEDED: u8 = 11;
pub const TEST_RETURN_ERROR: u8 = 12;
pub const TEST_PRIVILEGE_DEESCALATION_ESCALATION_SIGNER: u8 = 13;
pub const TEST_PRIVILEGE_DEESCALATION_ESCALATION_WRITABLE: u8 = 14;
pub const TEST_WRITABLE_DEESCALATION_WRITABLE: u8 = 15;
pub const TEST_NESTED_INVOKE_TOO_DEEP: u8 = 16;
pub const TEST_CALL_PRECOMPILE: u8 = 17;
pub const ADD_LAMPORTS: u8 = 18;
pub const TEST_RETURN_DATA_TOO_LARGE: u8 = 19;
pub const TEST_DUPLICATE_PRIVILEGE_ESCALATION_SIGNER: u8 = 20;
pub const TEST_DUPLICATE_PRIVILEGE_ESCALATION_WRITABLE: u8 = 21;
pub const TEST_MAX_ACCOUNT_INFOS_EXCEEDED: u8 = 22;
pub const TEST_FORBID_WRITE_AFTER_OWNERSHIP_CHANGE_IN_CALLEE: u8 = 23;
pub const TEST_FORBID_WRITE_AFTER_OWNERSHIP_CHANGE_IN_CALLEE_NESTED: u8 = 24;
pub const TEST_FORBID_WRITE_AFTER_OWNERSHIP_CHANGE_IN_CALLER: u8 = 25;
pub const TEST_FORBID_LEN_UPDATE_AFTER_OWNERSHIP_CHANGE_MOVING_DATA_POINTER: u8 = 26;
pub const TEST_FORBID_LEN_UPDATE_AFTER_OWNERSHIP_CHANGE: u8 = 27;
pub const TEST_ALLOW_WRITE_AFTER_OWNERSHIP_CHANGE_TO_CALLER: u8 = 28;
pub const TEST_CPI_ACCOUNT_UPDATE_CALLER_GROWS: u8 = 29;
pub const TEST_CPI_ACCOUNT_UPDATE_CALLER_GROWS_NESTED: u8 = 30;
pub const TEST_CPI_ACCOUNT_UPDATE_CALLEE_GROWS: u8 = 31;
pub const TEST_CPI_ACCOUNT_UPDATE_CALLEE_SHRINKS_SMALLER_THAN_ORIGINAL_LEN: u8 = 32;
pub const TEST_CPI_ACCOUNT_UPDATE_CALLER_GROWS_CALLEE_SHRINKS: u8 = 33;
pub const TEST_CPI_ACCOUNT_UPDATE_CALLER_GROWS_CALLEE_SHRINKS_NESTED: u8 = 34;
pub const TEST_CPI_INVALID_KEY_POINTER: u8 = 35;
pub const TEST_CPI_INVALID_OWNER_POINTER: u8 = 36;
pub const TEST_CPI_INVALID_LAMPORTS_POINTER: u8 = 37;
pub const TEST_CPI_INVALID_DATA_POINTER: u8 = 38;
pub const TEST_CPI_CHANGE_ACCOUNT_DATA_MEMORY_ALLOCATION: u8 = 39;
pub const TEST_WRITE_ACCOUNT: u8 = 40;
pub const TEST_CALLEE_ACCOUNT_UPDATES: u8 = 41;
pub const TEST_STACK_HEAP_ZEROED: u8 = 42;
pub const TEST_ACCOUNT_INFO_IN_ACCOUNT: u8 = 43;

pub const MINT_INDEX: usize = 0;
pub const ARGUMENT_INDEX: usize = 1;
pub const INVOKED_PROGRAM_INDEX: usize = 2;
pub const INVOKED_ARGUMENT_INDEX: usize = 3;
pub const INVOKED_PROGRAM_DUP_INDEX: usize = 4;
pub const ARGUMENT_DUP_INDEX: usize = 5;
pub const DERIVED_KEY1_INDEX: usize = 6;
pub const DERIVED_KEY2_INDEX: usize = 7;
pub const DERIVED_KEY3_INDEX: usize = 8;
pub const SYSTEM_PROGRAM_INDEX: usize = 9;
pub const FROM_INDEX: usize = 10;
pub const ED25519_PROGRAM_INDEX: usize = 11;
pub const INVOKE_PROGRAM_INDEX: usize = 12;
pub const UNEXECUTABLE_PROGRAM_INDEX: usize = 13;
