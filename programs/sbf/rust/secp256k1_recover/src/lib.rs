#![allow(clippy::arithmetic_side_effects)]
//! Secp256k1Recover Syscall test

extern crate solana_program;
use solana_program::{
    custom_heap_default, custom_panic_default, msg, secp256k1_recover::secp256k1_recover,
};

fn test_secp256k1_recover() {
    let expected: [u8; 64] = [
        0x42, 0xcd, 0x27, 0xe4, 0x0f, 0xdf, 0x7c, 0x97, 0x0a, 0xa2, 0xca, 0x0b, 0x88, 0x5b, 0x96,
        0x0f, 0x8b, 0x62, 0x8a, 0x41, 0xa1, 0x81, 0xe7, 0xe6, 0x8e, 0x03, 0xea, 0x0b, 0x84, 0x20,
        0x58, 0x9b, 0x32, 0x06, 0xbd, 0x66, 0x2f, 0x75, 0x65, 0xd6, 0x9d, 0xbd, 0x1d, 0x34, 0x29,
        0x6a, 0xd9, 0x35, 0x38, 0xed, 0x86, 0x9e, 0x99, 0x20, 0x43, 0xc3, 0xeb, 0xad, 0x65, 0x50,
        0xa0, 0x11, 0x6e, 0x5d,
    ];

    let hash: [u8; 32] = [
        0xde, 0xa5, 0x66, 0xb6, 0x94, 0x3b, 0xe0, 0xe9, 0x62, 0x53, 0xc2, 0x21, 0x5b, 0x1b, 0xac,
        0x69, 0xe7, 0xa8, 0x1e, 0xdb, 0x41, 0xc5, 0x02, 0x8b, 0x4f, 0x5c, 0x45, 0xc5, 0x3b, 0x49,
        0x54, 0xd0,
    ];
    let recovery_id: u8 = 1;
    let signature: [u8; 64] = [
        0x97, 0xa4, 0xee, 0x31, 0xfe, 0x82, 0x65, 0x72, 0x9f, 0x4a, 0xa6, 0x7d, 0x24, 0xd4, 0xa7,
        0x27, 0xf8, 0xc3, 0x15, 0xa4, 0xc8, 0xf9, 0x80, 0xeb, 0x4c, 0x4d, 0x4a, 0xfa, 0x6e, 0xc9,
        0x42, 0x41, 0x5d, 0x10, 0xd9, 0xc2, 0x8a, 0x90, 0xe9, 0x92, 0x9c, 0x52, 0x4b, 0x2c, 0xfb,
        0x65, 0xdf, 0xbc, 0xf6, 0x8c, 0xfd, 0x68, 0xdb, 0x17, 0xf9, 0x5d, 0x23, 0x5f, 0x96, 0xd8,
        0xf0, 0x72, 0x01, 0x2d,
    ];

    let public_key = secp256k1_recover(&hash[..], recovery_id, &signature[..]).unwrap();
    assert_eq!(public_key.to_bytes(), expected);
}

/// secp256k1_recover allows malleable signatures
fn test_secp256k1_recover_malleability() {
    let message = b"hello world";
    let message_hash = {
        let mut hasher = solana_program::keccak::Hasher::default();
        hasher.hash(message);
        hasher.result()
    };

    let pubkey_bytes: [u8; 64] = [
        0x9B, 0xEE, 0x7C, 0x18, 0x34, 0xE0, 0x18, 0x21, 0x7B, 0x40, 0x14, 0x9B, 0x84, 0x2E, 0xFA,
        0x80, 0x96, 0x00, 0x1A, 0x9B, 0x17, 0x88, 0x01, 0x80, 0xA8, 0x46, 0x99, 0x09, 0xE9, 0xC4,
        0x73, 0x6E, 0x39, 0x0B, 0x94, 0x00, 0x97, 0x68, 0xC2, 0x28, 0xB5, 0x55, 0xD3, 0x0C, 0x0C,
        0x42, 0x43, 0xC1, 0xEE, 0xA5, 0x0D, 0xC0, 0x48, 0x62, 0xD3, 0xAE, 0xB0, 0x3D, 0xA2, 0x20,
        0xAC, 0x11, 0x85, 0xEE,
    ];
    let signature_bytes: [u8; 64] = [
        0x93, 0x92, 0xC4, 0x6C, 0x42, 0xF6, 0x31, 0x73, 0x81, 0xD4, 0xB2, 0x44, 0xE9, 0x2F, 0xFC,
        0xE3, 0xF4, 0x57, 0xDD, 0x50, 0xB3, 0xA5, 0x20, 0x26, 0x3B, 0xE7, 0xEF, 0x8A, 0xB0, 0x69,
        0xBB, 0xDE, 0x2F, 0x90, 0x12, 0x93, 0xD7, 0x3F, 0xA0, 0x29, 0x0C, 0x46, 0x4B, 0x97, 0xC5,
        0x00, 0xAD, 0xEA, 0x6A, 0x64, 0x4D, 0xC3, 0x8D, 0x25, 0x24, 0xEF, 0x97, 0x6D, 0xC6, 0xD7,
        0x1D, 0x9F, 0x5A, 0x26,
    ];
    let recovery_id: u8 = 0;

    let signature = libsecp256k1::Signature::parse_standard_slice(&signature_bytes).unwrap();

    // Flip the S value in the signature to make a different but valid signature.
    let mut alt_signature = signature;
    alt_signature.s = -alt_signature.s;
    let alt_recovery_id = libsecp256k1::RecoveryId::parse(recovery_id ^ 1).unwrap();

    let alt_signature_bytes = alt_signature.serialize();
    let alt_recovery_id = alt_recovery_id.serialize();

    let recovered_pubkey =
        secp256k1_recover(&message_hash.0, recovery_id, &signature_bytes[..]).unwrap();
    assert_eq!(recovered_pubkey.to_bytes(), pubkey_bytes);

    let alt_recovered_pubkey =
        secp256k1_recover(&message_hash.0, alt_recovery_id, &alt_signature_bytes[..]).unwrap();
    assert_eq!(alt_recovered_pubkey.to_bytes(), pubkey_bytes);
}

#[no_mangle]
pub extern "C" fn entrypoint(_input: *mut u8) -> u64 {
    msg!("secp256k1_recover");

    test_secp256k1_recover();
    test_secp256k1_recover_malleability();

    0
}

custom_heap_default!();
custom_panic_default!();
