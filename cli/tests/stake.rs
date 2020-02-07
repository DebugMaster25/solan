use solana_cli::{
    cli::{process_command, request_and_confirm_airdrop, CliCommand, CliConfig},
    offline::{parse_sign_only_reply_string, BlockhashQuery},
};
use solana_client::rpc_client::RpcClient;
use solana_faucet::faucet::run_local_faucet;
use solana_sdk::{
    account_utils::StateMut,
    fee_calculator::FeeCalculator,
    nonce_state::NonceState,
    pubkey::Pubkey,
    signature::{read_keypair_file, write_keypair, Keypair, KeypairUtil},
    system_instruction::create_address_with_seed,
};
use solana_stake_program::stake_state::{Lockup, StakeAuthorize, StakeState};
use std::fs::remove_dir_all;
use std::sync::mpsc::channel;

#[cfg(test)]
use solana_core::validator::{new_validator_for_tests, new_validator_for_tests_ex};
use std::thread::sleep;
use std::time::Duration;

use tempfile::NamedTempFile;

fn make_tmp_file() -> (String, NamedTempFile) {
    let tmp_file = NamedTempFile::new().unwrap();
    (String::from(tmp_file.path().to_str().unwrap()), tmp_file)
}

fn check_balance(expected_balance: u64, client: &RpcClient, pubkey: &Pubkey) {
    (0..5).for_each(|tries| {
        let balance = client.retry_get_balance(pubkey, 1).unwrap().unwrap();
        if balance == expected_balance {
            return;
        }
        if tries == 4 {
            assert_eq!(balance, expected_balance);
        }
        sleep(Duration::from_millis(500));
    });
}

#[test]
fn test_seed_stake_delegation_and_deactivation() {
    solana_logger::setup();

    let (server, leader_data, alice, ledger_path) = new_validator_for_tests();
    let (sender, receiver) = channel();
    run_local_faucet(alice, sender, None);
    let faucet_addr = receiver.recv().unwrap();

    let rpc_client = RpcClient::new_socket(leader_data.rpc);

    let mut config_validator = CliConfig::default();
    config_validator.json_rpc_url =
        format!("http://{}:{}", leader_data.rpc.ip(), leader_data.rpc.port());

    let (validator_keypair_file, mut tmp_file) = make_tmp_file();
    write_keypair(&config_validator.keypair, tmp_file.as_file_mut()).unwrap();

    let mut config_vote = CliConfig::default();
    config_vote.json_rpc_url =
        format!("http://{}:{}", leader_data.rpc.ip(), leader_data.rpc.port());
    let (vote_keypair_file, mut tmp_file) = make_tmp_file();
    write_keypair(&config_vote.keypair, tmp_file.as_file_mut()).unwrap();

    let mut config_stake = CliConfig::default();
    config_stake.json_rpc_url =
        format!("http://{}:{}", leader_data.rpc.ip(), leader_data.rpc.port());

    request_and_confirm_airdrop(
        &rpc_client,
        &faucet_addr,
        &config_validator.keypair.pubkey(),
        100_000,
    )
    .unwrap();
    check_balance(100_000, &rpc_client, &config_validator.keypair.pubkey());

    // Create vote account
    config_validator.command = CliCommand::CreateVoteAccount {
        vote_account: read_keypair_file(&vote_keypair_file).unwrap().into(),
        seed: None,
        node_pubkey: config_validator.keypair.pubkey(),
        authorized_voter: None,
        authorized_withdrawer: None,
        commission: 0,
    };
    process_command(&config_validator).unwrap();

    let stake_address = create_address_with_seed(
        &config_validator.keypair.pubkey(),
        "hi there",
        &solana_stake_program::id(),
    )
    .expect("bad seed");

    // Create stake account with a seed, uses the validator config as the base,
    //   which is nice ;)
    config_validator.command = CliCommand::CreateStakeAccount {
        stake_account: read_keypair_file(&validator_keypair_file).unwrap().into(),
        seed: Some("hi there".to_string()),
        staker: None,
        withdrawer: None,
        lockup: Lockup::default(),
        lamports: 50_000,
    };
    process_command(&config_validator).unwrap();

    // Delegate stake
    config_validator.command = CliCommand::DelegateStake {
        stake_account_pubkey: stake_address,
        vote_account_pubkey: config_vote.keypair.pubkey(),
        stake_authority: None,
        force: true,
        sign_only: false,
        signers: None,
        blockhash_query: BlockhashQuery::default(),
        nonce_account: None,
        nonce_authority: None,
        fee_payer: None,
    };
    process_command(&config_validator).unwrap();

    // Deactivate stake
    config_validator.command = CliCommand::DeactivateStake {
        stake_account_pubkey: stake_address,
        stake_authority: None,
        sign_only: false,
        signers: None,
        blockhash_query: BlockhashQuery::default(),
        nonce_account: None,
        nonce_authority: None,
        fee_payer: None,
    };
    process_command(&config_validator).unwrap();

    server.close().unwrap();
    remove_dir_all(ledger_path).unwrap();
}

#[test]
fn test_stake_delegation_and_deactivation() {
    solana_logger::setup();

    let (server, leader_data, alice, ledger_path) = new_validator_for_tests();
    let (sender, receiver) = channel();
    run_local_faucet(alice, sender, None);
    let faucet_addr = receiver.recv().unwrap();

    let rpc_client = RpcClient::new_socket(leader_data.rpc);

    let mut config_validator = CliConfig::default();
    config_validator.json_rpc_url =
        format!("http://{}:{}", leader_data.rpc.ip(), leader_data.rpc.port());

    let mut config_vote = CliConfig::default();
    config_vote.json_rpc_url =
        format!("http://{}:{}", leader_data.rpc.ip(), leader_data.rpc.port());
    let (vote_keypair_file, mut tmp_file) = make_tmp_file();
    write_keypair(&config_vote.keypair, tmp_file.as_file_mut()).unwrap();

    let mut config_stake = CliConfig::default();
    config_stake.json_rpc_url =
        format!("http://{}:{}", leader_data.rpc.ip(), leader_data.rpc.port());
    let (stake_keypair_file, mut tmp_file) = make_tmp_file();
    write_keypair(&config_stake.keypair, tmp_file.as_file_mut()).unwrap();

    request_and_confirm_airdrop(
        &rpc_client,
        &faucet_addr,
        &config_validator.keypair.pubkey(),
        100_000,
    )
    .unwrap();
    check_balance(100_000, &rpc_client, &config_validator.keypair.pubkey());

    // Create vote account
    config_validator.command = CliCommand::CreateVoteAccount {
        vote_account: read_keypair_file(&vote_keypair_file).unwrap().into(),
        seed: None,
        node_pubkey: config_validator.keypair.pubkey(),
        authorized_voter: None,
        authorized_withdrawer: None,
        commission: 0,
    };
    process_command(&config_validator).unwrap();

    // Create stake account
    config_validator.command = CliCommand::CreateStakeAccount {
        stake_account: read_keypair_file(&stake_keypair_file).unwrap().into(),
        seed: None,
        staker: None,
        withdrawer: None,
        lockup: Lockup::default(),
        lamports: 50_000,
    };
    process_command(&config_validator).unwrap();

    // Delegate stake
    config_validator.command = CliCommand::DelegateStake {
        stake_account_pubkey: config_stake.keypair.pubkey(),
        vote_account_pubkey: config_vote.keypair.pubkey(),
        stake_authority: None,
        force: true,
        sign_only: false,
        signers: None,
        blockhash_query: BlockhashQuery::default(),
        nonce_account: None,
        nonce_authority: None,
        fee_payer: None,
    };
    process_command(&config_validator).unwrap();

    // Deactivate stake
    config_validator.command = CliCommand::DeactivateStake {
        stake_account_pubkey: config_stake.keypair.pubkey(),
        stake_authority: None,
        sign_only: false,
        signers: None,
        blockhash_query: BlockhashQuery::default(),
        nonce_account: None,
        nonce_authority: None,
        fee_payer: None,
    };
    process_command(&config_validator).unwrap();

    server.close().unwrap();
    remove_dir_all(ledger_path).unwrap();
}

#[test]
fn test_offline_stake_delegation_and_deactivation() {
    solana_logger::setup();

    let (server, leader_data, alice, ledger_path) = new_validator_for_tests();
    let (sender, receiver) = channel();
    run_local_faucet(alice, sender, None);
    let faucet_addr = receiver.recv().unwrap();

    let rpc_client = RpcClient::new_socket(leader_data.rpc);

    let mut config_validator = CliConfig::default();
    config_validator.json_rpc_url =
        format!("http://{}:{}", leader_data.rpc.ip(), leader_data.rpc.port());

    let mut config_payer = CliConfig::default();
    config_payer.json_rpc_url =
        format!("http://{}:{}", leader_data.rpc.ip(), leader_data.rpc.port());

    let mut config_vote = CliConfig::default();
    config_vote.json_rpc_url =
        format!("http://{}:{}", leader_data.rpc.ip(), leader_data.rpc.port());
    let (vote_keypair_file, mut tmp_file) = make_tmp_file();
    write_keypair(&config_vote.keypair, tmp_file.as_file_mut()).unwrap();

    let mut config_stake = CliConfig::default();
    config_stake.json_rpc_url =
        format!("http://{}:{}", leader_data.rpc.ip(), leader_data.rpc.port());
    let (stake_keypair_file, mut tmp_file) = make_tmp_file();
    write_keypair(&config_stake.keypair, tmp_file.as_file_mut()).unwrap();

    request_and_confirm_airdrop(
        &rpc_client,
        &faucet_addr,
        &config_validator.keypair.pubkey(),
        100_000,
    )
    .unwrap();
    check_balance(100_000, &rpc_client, &config_validator.keypair.pubkey());

    // Create vote account
    config_validator.command = CliCommand::CreateVoteAccount {
        vote_account: read_keypair_file(&vote_keypair_file).unwrap().into(),
        seed: None,
        node_pubkey: config_validator.keypair.pubkey(),
        authorized_voter: None,
        authorized_withdrawer: None,
        commission: 0,
    };
    process_command(&config_validator).unwrap();

    // Create stake account
    config_validator.command = CliCommand::CreateStakeAccount {
        stake_account: read_keypair_file(&stake_keypair_file).unwrap().into(),
        seed: None,
        staker: None,
        withdrawer: None,
        lockup: Lockup::default(),
        lamports: 50_000,
    };
    process_command(&config_validator).unwrap();

    // Delegate stake offline
    let (blockhash, _) = rpc_client.get_recent_blockhash().unwrap();
    config_validator.command = CliCommand::DelegateStake {
        stake_account_pubkey: config_stake.keypair.pubkey(),
        vote_account_pubkey: config_vote.keypair.pubkey(),
        stake_authority: None,
        force: true,
        sign_only: true,
        signers: None,
        blockhash_query: BlockhashQuery::None(blockhash, FeeCalculator::default()),
        nonce_account: None,
        nonce_authority: None,
        fee_payer: None,
    };
    let sig_response = process_command(&config_validator).unwrap();
    let (blockhash, signers) = parse_sign_only_reply_string(&sig_response);

    // Delegate stake online
    config_payer.command = CliCommand::DelegateStake {
        stake_account_pubkey: config_stake.keypair.pubkey(),
        vote_account_pubkey: config_vote.keypair.pubkey(),
        stake_authority: None,
        force: true,
        sign_only: false,
        signers: Some(signers),
        blockhash_query: BlockhashQuery::None(blockhash, FeeCalculator::default()),
        nonce_account: None,
        nonce_authority: None,
        fee_payer: None,
    };
    process_command(&config_payer).unwrap();

    // Deactivate stake offline
    let (blockhash, _) = rpc_client.get_recent_blockhash().unwrap();
    config_validator.command = CliCommand::DeactivateStake {
        stake_account_pubkey: config_stake.keypair.pubkey(),
        stake_authority: None,
        sign_only: true,
        signers: None,
        blockhash_query: BlockhashQuery::None(blockhash, FeeCalculator::default()),
        nonce_account: None,
        nonce_authority: None,
        fee_payer: None,
    };
    let sig_response = process_command(&config_validator).unwrap();
    let (blockhash, signers) = parse_sign_only_reply_string(&sig_response);

    // Deactivate stake online
    config_payer.command = CliCommand::DeactivateStake {
        stake_account_pubkey: config_stake.keypair.pubkey(),
        stake_authority: None,
        sign_only: false,
        signers: Some(signers),
        blockhash_query: BlockhashQuery::FeeCalculator(blockhash),
        nonce_account: None,
        nonce_authority: None,
        fee_payer: None,
    };
    process_command(&config_payer).unwrap();

    server.close().unwrap();
    remove_dir_all(ledger_path).unwrap();
}

#[test]
fn test_nonced_stake_delegation_and_deactivation() {
    solana_logger::setup();

    let (server, leader_data, alice, ledger_path) = new_validator_for_tests();
    let (sender, receiver) = channel();
    run_local_faucet(alice, sender, None);
    let faucet_addr = receiver.recv().unwrap();

    let rpc_client = RpcClient::new_socket(leader_data.rpc);

    let mut config = CliConfig::default();
    config.json_rpc_url = format!("http://{}:{}", leader_data.rpc.ip(), leader_data.rpc.port());

    let minimum_nonce_balance = rpc_client
        .get_minimum_balance_for_rent_exemption(NonceState::size())
        .unwrap();

    request_and_confirm_airdrop(&rpc_client, &faucet_addr, &config.keypair.pubkey(), 100_000)
        .unwrap();

    // Create vote account
    let vote_keypair = Keypair::new();
    let (vote_keypair_file, mut tmp_file) = make_tmp_file();
    write_keypair(&vote_keypair, tmp_file.as_file_mut()).unwrap();
    config.command = CliCommand::CreateVoteAccount {
        vote_account: read_keypair_file(&vote_keypair_file).unwrap().into(),
        seed: None,
        node_pubkey: config.keypair.pubkey(),
        authorized_voter: None,
        authorized_withdrawer: None,
        commission: 0,
    };
    process_command(&config).unwrap();

    // Create stake account
    let stake_keypair = Keypair::new();
    let (stake_keypair_file, mut tmp_file) = make_tmp_file();
    write_keypair(&stake_keypair, tmp_file.as_file_mut()).unwrap();
    config.command = CliCommand::CreateStakeAccount {
        stake_account: read_keypair_file(&stake_keypair_file).unwrap().into(),
        seed: None,
        staker: None,
        withdrawer: None,
        lockup: Lockup::default(),
        lamports: 50_000,
    };
    process_command(&config).unwrap();

    // Create nonce account
    let nonce_account = Keypair::new();
    let (nonce_keypair_file, mut tmp_file) = make_tmp_file();
    write_keypair(&nonce_account, tmp_file.as_file_mut()).unwrap();
    config.command = CliCommand::CreateNonceAccount {
        nonce_account: read_keypair_file(&nonce_keypair_file).unwrap().into(),
        seed: None,
        nonce_authority: Some(config.keypair.pubkey()),
        lamports: minimum_nonce_balance,
    };
    process_command(&config).unwrap();

    // Fetch nonce hash
    let account = rpc_client.get_account(&nonce_account.pubkey()).unwrap();
    let nonce_state: NonceState = account.state().unwrap();
    let nonce_hash = match nonce_state {
        NonceState::Initialized(_meta, hash) => hash,
        _ => panic!("Nonce is not initialized"),
    };

    // Delegate stake
    config.command = CliCommand::DelegateStake {
        stake_account_pubkey: stake_keypair.pubkey(),
        vote_account_pubkey: vote_keypair.pubkey(),
        stake_authority: None,
        force: true,
        sign_only: false,
        signers: None,
        blockhash_query: BlockhashQuery::None(nonce_hash, FeeCalculator::default()),
        nonce_account: Some(nonce_account.pubkey()),
        nonce_authority: None,
        fee_payer: None,
    };
    process_command(&config).unwrap();

    // Fetch nonce hash
    let account = rpc_client.get_account(&nonce_account.pubkey()).unwrap();
    let nonce_state: NonceState = account.state().unwrap();
    let nonce_hash = match nonce_state {
        NonceState::Initialized(_meta, hash) => hash,
        _ => panic!("Nonce is not initialized"),
    };

    // Deactivate stake
    let config_keypair = Keypair::from_bytes(&config.keypair.to_bytes()).unwrap();
    config.command = CliCommand::DeactivateStake {
        stake_account_pubkey: stake_keypair.pubkey(),
        stake_authority: None,
        sign_only: false,
        signers: None,
        blockhash_query: BlockhashQuery::FeeCalculator(nonce_hash),
        nonce_account: Some(nonce_account.pubkey()),
        nonce_authority: Some(config_keypair.into()),
        fee_payer: None,
    };
    process_command(&config).unwrap();

    server.close().unwrap();
    remove_dir_all(ledger_path).unwrap();
}

#[test]
fn test_stake_authorize() {
    solana_logger::setup();

    let (server, leader_data, alice, ledger_path) = new_validator_for_tests();
    let (sender, receiver) = channel();
    run_local_faucet(alice, sender, None);
    let faucet_addr = receiver.recv().unwrap();

    let rpc_client = RpcClient::new_socket(leader_data.rpc);

    let mut config = CliConfig::default();
    config.json_rpc_url = format!("http://{}:{}", leader_data.rpc.ip(), leader_data.rpc.port());

    request_and_confirm_airdrop(&rpc_client, &faucet_addr, &config.keypair.pubkey(), 100_000)
        .unwrap();

    // Create stake account, identity is authority
    let stake_keypair = Keypair::new();
    let stake_account_pubkey = stake_keypair.pubkey();
    let (stake_keypair_file, mut tmp_file) = make_tmp_file();
    write_keypair(&stake_keypair, tmp_file.as_file_mut()).unwrap();
    config.command = CliCommand::CreateStakeAccount {
        stake_account: read_keypair_file(&stake_keypair_file).unwrap().into(),
        seed: None,
        staker: None,
        withdrawer: None,
        lockup: Lockup::default(),
        lamports: 50_000,
    };
    process_command(&config).unwrap();

    // Assign new online stake authority
    let online_authority = Keypair::new();
    let online_authority_pubkey = online_authority.pubkey();
    let (online_authority_file, mut tmp_file) = make_tmp_file();
    write_keypair(&online_authority, tmp_file.as_file_mut()).unwrap();
    config.command = CliCommand::StakeAuthorize {
        stake_account_pubkey,
        new_authorized_pubkey: online_authority_pubkey,
        stake_authorize: StakeAuthorize::Staker,
        authority: None,
        sign_only: false,
        signers: None,
        blockhash_query: BlockhashQuery::default(),
        nonce_account: None,
        nonce_authority: None,
        fee_payer: None,
    };
    process_command(&config).unwrap();
    let stake_account = rpc_client.get_account(&stake_account_pubkey).unwrap();
    let stake_state: StakeState = stake_account.state().unwrap();
    let current_authority = match stake_state {
        StakeState::Initialized(meta) => meta.authorized.staker,
        _ => panic!("Unexpected stake state!"),
    };
    assert_eq!(current_authority, online_authority_pubkey);

    // Assign new offline stake authority
    let offline_authority = Keypair::new();
    let offline_authority_pubkey = offline_authority.pubkey();
    let (offline_authority_file, mut tmp_file) = make_tmp_file();
    write_keypair(&offline_authority, tmp_file.as_file_mut()).unwrap();
    config.command = CliCommand::StakeAuthorize {
        stake_account_pubkey,
        new_authorized_pubkey: offline_authority_pubkey,
        stake_authorize: StakeAuthorize::Staker,
        authority: Some(read_keypair_file(&online_authority_file).unwrap().into()),
        sign_only: false,
        signers: None,
        blockhash_query: BlockhashQuery::default(),
        nonce_account: None,
        nonce_authority: None,
        fee_payer: None,
    };
    process_command(&config).unwrap();
    let stake_account = rpc_client.get_account(&stake_account_pubkey).unwrap();
    let stake_state: StakeState = stake_account.state().unwrap();
    let current_authority = match stake_state {
        StakeState::Initialized(meta) => meta.authorized.staker,
        _ => panic!("Unexpected stake state!"),
    };
    assert_eq!(current_authority, offline_authority_pubkey);

    // Offline assignment of new nonced stake authority
    let nonced_authority = Keypair::new();
    let nonced_authority_pubkey = nonced_authority.pubkey();
    let (nonced_authority_file, mut tmp_file) = make_tmp_file();
    write_keypair(&nonced_authority, tmp_file.as_file_mut()).unwrap();
    let (blockhash, _) = rpc_client.get_recent_blockhash().unwrap();
    config.command = CliCommand::StakeAuthorize {
        stake_account_pubkey,
        new_authorized_pubkey: nonced_authority_pubkey,
        stake_authorize: StakeAuthorize::Staker,
        authority: Some(read_keypair_file(&offline_authority_file).unwrap().into()),
        sign_only: true,
        signers: None,
        blockhash_query: BlockhashQuery::None(blockhash, FeeCalculator::default()),
        nonce_account: None,
        nonce_authority: None,
        fee_payer: None,
    };
    let sign_reply = process_command(&config).unwrap();
    let (blockhash, signers) = parse_sign_only_reply_string(&sign_reply);
    config.command = CliCommand::StakeAuthorize {
        stake_account_pubkey,
        new_authorized_pubkey: nonced_authority_pubkey,
        stake_authorize: StakeAuthorize::Staker,
        authority: Some(offline_authority_pubkey.into()),
        sign_only: false,
        signers: Some(signers),
        blockhash_query: BlockhashQuery::FeeCalculator(blockhash),
        nonce_account: None,
        nonce_authority: None,
        fee_payer: None,
    };
    process_command(&config).unwrap();
    let stake_account = rpc_client.get_account(&stake_account_pubkey).unwrap();
    let stake_state: StakeState = stake_account.state().unwrap();
    let current_authority = match stake_state {
        StakeState::Initialized(meta) => meta.authorized.staker,
        _ => panic!("Unexpected stake state!"),
    };
    assert_eq!(current_authority, nonced_authority_pubkey);

    // Create nonce account
    let minimum_nonce_balance = rpc_client
        .get_minimum_balance_for_rent_exemption(NonceState::size())
        .unwrap();
    let nonce_account = Keypair::new();
    let (nonce_keypair_file, mut tmp_file) = make_tmp_file();
    write_keypair(&nonce_account, tmp_file.as_file_mut()).unwrap();
    config.command = CliCommand::CreateNonceAccount {
        nonce_account: read_keypair_file(&nonce_keypair_file).unwrap().into(),
        seed: None,
        nonce_authority: Some(config.keypair.pubkey()),
        lamports: minimum_nonce_balance,
    };
    process_command(&config).unwrap();

    // Fetch nonce hash
    let account = rpc_client.get_account(&nonce_account.pubkey()).unwrap();
    let nonce_state: NonceState = account.state().unwrap();
    let nonce_hash = match nonce_state {
        NonceState::Initialized(_meta, hash) => hash,
        _ => panic!("Nonce is not initialized"),
    };

    // Nonced assignment of new online stake authority
    let online_authority = Keypair::new();
    let online_authority_pubkey = online_authority.pubkey();
    let (_online_authority_file, mut tmp_file) = make_tmp_file();
    write_keypair(&online_authority, tmp_file.as_file_mut()).unwrap();
    config.command = CliCommand::StakeAuthorize {
        stake_account_pubkey,
        new_authorized_pubkey: online_authority_pubkey,
        stake_authorize: StakeAuthorize::Staker,
        authority: Some(read_keypair_file(&nonced_authority_file).unwrap().into()),
        sign_only: true,
        signers: None,
        blockhash_query: BlockhashQuery::None(nonce_hash, FeeCalculator::default()),
        nonce_account: Some(nonce_account.pubkey()),
        nonce_authority: None,
        fee_payer: None,
    };
    let sign_reply = process_command(&config).unwrap();
    let (blockhash, signers) = parse_sign_only_reply_string(&sign_reply);
    assert_eq!(blockhash, nonce_hash);
    config.command = CliCommand::StakeAuthorize {
        stake_account_pubkey,
        new_authorized_pubkey: online_authority_pubkey,
        stake_authorize: StakeAuthorize::Staker,
        authority: Some(nonced_authority_pubkey.into()),
        sign_only: false,
        signers: Some(signers),
        blockhash_query: BlockhashQuery::FeeCalculator(blockhash),
        nonce_account: Some(nonce_account.pubkey()),
        nonce_authority: None,
        fee_payer: None,
    };
    process_command(&config).unwrap();
    let stake_account = rpc_client.get_account(&stake_account_pubkey).unwrap();
    let stake_state: StakeState = stake_account.state().unwrap();
    let current_authority = match stake_state {
        StakeState::Initialized(meta) => meta.authorized.staker,
        _ => panic!("Unexpected stake state!"),
    };
    assert_eq!(current_authority, online_authority_pubkey);
    let account = rpc_client.get_account(&nonce_account.pubkey()).unwrap();
    let nonce_state: NonceState = account.state().unwrap();
    let new_nonce_hash = match nonce_state {
        NonceState::Initialized(_meta, hash) => hash,
        _ => panic!("Nonce is not initialized"),
    };
    assert_ne!(nonce_hash, new_nonce_hash);

    server.close().unwrap();
    remove_dir_all(ledger_path).unwrap();
}

#[test]
fn test_stake_authorize_with_fee_payer() {
    solana_logger::setup();
    const SIG_FEE: u64 = 42;

    let (server, leader_data, alice, ledger_path) = new_validator_for_tests_ex(SIG_FEE, 42_000);
    let (sender, receiver) = channel();
    run_local_faucet(alice, sender, None);
    let faucet_addr = receiver.recv().unwrap();

    let rpc_client = RpcClient::new_socket(leader_data.rpc);

    let mut config = CliConfig::default();
    config.json_rpc_url = format!("http://{}:{}", leader_data.rpc.ip(), leader_data.rpc.port());

    let mut config_payer = CliConfig::default();
    config_payer.json_rpc_url =
        format!("http://{}:{}", leader_data.rpc.ip(), leader_data.rpc.port());
    let payer_pubkey = config_payer.keypair.pubkey();
    let (payer_keypair_file, mut tmp_file) = make_tmp_file();
    write_keypair(&config_payer.keypair, tmp_file.as_file_mut()).unwrap();

    let mut config_offline = CliConfig::default();
    let offline_pubkey = config_offline.keypair.pubkey();
    let (_offline_keypair_file, mut tmp_file) = make_tmp_file();
    write_keypair(&config_offline.keypair, tmp_file.as_file_mut()).unwrap();
    // Verify we're offline
    config_offline.command = CliCommand::ClusterVersion;
    process_command(&config_offline).unwrap_err();

    request_and_confirm_airdrop(&rpc_client, &faucet_addr, &config.keypair.pubkey(), 100_000)
        .unwrap();
    check_balance(100_000, &rpc_client, &config.keypair.pubkey());

    request_and_confirm_airdrop(&rpc_client, &faucet_addr, &payer_pubkey, 100_000).unwrap();
    check_balance(100_000, &rpc_client, &payer_pubkey);

    request_and_confirm_airdrop(&rpc_client, &faucet_addr, &offline_pubkey, 100_000).unwrap();
    check_balance(100_000, &rpc_client, &offline_pubkey);

    // Create stake account, identity is authority
    let stake_keypair = Keypair::new();
    let stake_account_pubkey = stake_keypair.pubkey();
    let (stake_keypair_file, mut tmp_file) = make_tmp_file();
    write_keypair(&stake_keypair, tmp_file.as_file_mut()).unwrap();
    config.command = CliCommand::CreateStakeAccount {
        stake_account: read_keypair_file(&stake_keypair_file).unwrap().into(),
        seed: None,
        staker: None,
        withdrawer: None,
        lockup: Lockup::default(),
        lamports: 50_000,
    };
    process_command(&config).unwrap();
    // `config` balance should be 50,000 - 1 stake account sig - 1 fee sig
    check_balance(
        50_000 - SIG_FEE - SIG_FEE,
        &rpc_client,
        &config.keypair.pubkey(),
    );

    // Assign authority with separate fee payer
    config.command = CliCommand::StakeAuthorize {
        stake_account_pubkey,
        new_authorized_pubkey: offline_pubkey,
        stake_authorize: StakeAuthorize::Staker,
        authority: None,
        sign_only: false,
        signers: None,
        blockhash_query: BlockhashQuery::All,
        nonce_account: None,
        nonce_authority: None,
        fee_payer: Some(read_keypair_file(&payer_keypair_file).unwrap().into()),
    };
    process_command(&config).unwrap();
    // `config` balance has not changed, despite submitting the TX
    check_balance(
        50_000 - SIG_FEE - SIG_FEE,
        &rpc_client,
        &config.keypair.pubkey(),
    );
    // `config_payer` however has paid `config`'s authority sig
    // and `config_payer`'s fee sig
    check_balance(
        100_000 - SIG_FEE - SIG_FEE,
        &rpc_client,
        &config_payer.keypair.pubkey(),
    );

    // Assign authority with offline fee payer
    let (blockhash, _) = rpc_client.get_recent_blockhash().unwrap();
    config_offline.command = CliCommand::StakeAuthorize {
        stake_account_pubkey,
        new_authorized_pubkey: payer_pubkey,
        stake_authorize: StakeAuthorize::Staker,
        authority: None,
        sign_only: true,
        signers: None,
        blockhash_query: BlockhashQuery::None(blockhash, FeeCalculator::default()),
        nonce_account: None,
        nonce_authority: None,
        fee_payer: None,
    };
    let sign_reply = process_command(&config_offline).unwrap();
    let (blockhash, signers) = parse_sign_only_reply_string(&sign_reply);
    config.command = CliCommand::StakeAuthorize {
        stake_account_pubkey,
        new_authorized_pubkey: payer_pubkey,
        stake_authorize: StakeAuthorize::Staker,
        authority: Some(offline_pubkey.into()),
        sign_only: false,
        signers: Some(signers),
        blockhash_query: BlockhashQuery::FeeCalculator(blockhash),
        nonce_account: None,
        nonce_authority: None,
        fee_payer: Some(offline_pubkey.into()),
    };
    process_command(&config).unwrap();
    // `config`'s balance again has not changed
    check_balance(
        50_000 - SIG_FEE - SIG_FEE,
        &rpc_client,
        &config.keypair.pubkey(),
    );
    // `config_offline` however has paid 1 sig due to being both authority
    // and fee payer
    check_balance(
        100_000 - SIG_FEE,
        &rpc_client,
        &config_offline.keypair.pubkey(),
    );

    server.close().unwrap();
    remove_dir_all(ledger_path).unwrap();
}
