#![allow(incomplete_features)]
#![cfg_attr(RUSTC_WITH_SPECIALIZATION, feature(specialization))]
#![cfg_attr(RUSTC_NEEDS_PROC_MACRO_HYGIENE, feature(proc_macro_hygiene))]

pub mod processor;

#[deprecated(
    since = "1.17.0",
    note = "Please use `solana_sdk::address_lookup_table` instead"
)]
pub use solana_sdk::address_lookup_table::{
    error, instruction,
    program::{check_id, id, ID},
    state,
};
