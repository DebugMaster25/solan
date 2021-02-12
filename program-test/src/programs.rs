use solana_sdk::{account::Account, pubkey::Pubkey, rent::Rent};

mod spl_token {
    solana_sdk::declare_id!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
}
mod spl_memo {
    solana_sdk::declare_id!("Memo1UhkJRfHyvLMcVucJwxXeuD728EqVDDwQDxFMNo");
}
mod spl_associated_token_account {
    solana_sdk::declare_id!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
}

static SPL_PROGRAMS: &[(Pubkey, &[u8])] = &[
    (spl_token::ID, include_bytes!("programs/spl_token-3.1.0.so")),
    (spl_memo::ID, include_bytes!("programs/spl_memo-1.0.0.so")),
    (
        spl_associated_token_account::ID,
        include_bytes!("programs/spl_associated-token-account-1.0.1.so"),
    ),
];

pub fn spl_programs(rent: &Rent) -> Vec<(Pubkey, Account)> {
    SPL_PROGRAMS
        .iter()
        .map(|(program_id, elf)| {
            (
                *program_id,
                Account {
                    lamports: rent.minimum_balance(elf.len()).min(1),
                    data: elf.to_vec(),
                    owner: solana_program::bpf_loader::id(),
                    executable: true,
                    rent_epoch: 0,
                },
            )
        })
        .collect()
}
