use {
    solana_banks_client::BanksClient,
    solana_program::{
        account_info::AccountInfo, entrypoint::ProgramResult, hash::Hash, instruction::Instruction,
        msg, pubkey::Pubkey, rent::Rent, system_instruction,
    },
    solana_program_test::{processor, ProgramTest},
    solana_sdk::{signature::Keypair, signature::Signer, transaction::Transaction},
};

#[allow(clippy::unnecessary_wraps)]
fn process_instruction(
    _program_id: &Pubkey,
    _accounts: &[AccountInfo],
    _input: &[u8],
) -> ProgramResult {
    msg!("Processing instruction");
    Ok(())
}

#[test]
fn simulate_fuzz() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let program_id = Pubkey::new_unique();
    // Initialize and start the test network
    let program_test = ProgramTest::new(
        "program-test-fuzz",
        program_id,
        processor!(process_instruction),
    );

    let (mut banks_client, payer, last_blockhash) =
        rt.block_on(async { program_test.start().await });

    // the honggfuzz `fuzz!` macro does not allow for async closures,
    // so we have to use the runtime directly to run async functions
    rt.block_on(async {
        run_fuzz_instructions(
            &[1, 2, 3, 4, 5],
            &mut banks_client,
            &payer,
            last_blockhash,
            &program_id,
        )
        .await
    });
}

#[test]
fn simulate_fuzz_with_context() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let program_id = Pubkey::new_unique();
    // Initialize and start the test network
    let program_test = ProgramTest::new(
        "program-test-fuzz",
        program_id,
        processor!(process_instruction),
    );

    let mut test_state = rt.block_on(async { program_test.start_with_context().await });

    // the honggfuzz `fuzz!` macro does not allow for async closures,
    // so we have to use the runtime directly to run async functions
    rt.block_on(async {
        run_fuzz_instructions(
            &[1, 2, 3, 4, 5],
            &mut test_state.banks_client,
            &test_state.payer,
            test_state.last_blockhash,
            &program_id,
        )
        .await
    });
}

async fn run_fuzz_instructions(
    fuzz_instruction: &[u8],
    banks_client: &mut BanksClient,
    payer: &Keypair,
    last_blockhash: Hash,
    program_id: &Pubkey,
) {
    let mut instructions = vec![];
    let mut signer_keypairs = vec![];
    for &i in fuzz_instruction {
        let keypair = Keypair::new();
        let instruction = system_instruction::create_account(
            &payer.pubkey(),
            &keypair.pubkey(),
            Rent::default().minimum_balance(i as usize),
            i as u64,
            program_id,
        );
        instructions.push(instruction);
        instructions.push(Instruction::new(*program_id, &[0], vec![]));
        signer_keypairs.push(keypair);
    }
    // Process transaction on test network
    let mut transaction = Transaction::new_with_payer(&instructions, Some(&payer.pubkey()));
    let signers = [payer]
        .iter()
        .copied()
        .chain(signer_keypairs.iter())
        .collect::<Vec<&Keypair>>();
    transaction.partial_sign(&signers, last_blockhash);

    banks_client.process_transaction(transaction).await.unwrap();
    for keypair in signer_keypairs {
        let account = banks_client
            .get_account(keypair.pubkey())
            .await
            .expect("account exists")
            .unwrap();
        assert!(account.lamports > 0);
        assert!(!account.data.is_empty());
    }
}
