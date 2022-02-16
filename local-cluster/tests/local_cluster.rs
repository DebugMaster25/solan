#![allow(clippy::integer_arithmetic)]
use {
    assert_matches::assert_matches,
    common::{
        copy_blocks, create_custom_leader_schedule_with_random_keys, last_vote_in_tower,
        ms_for_n_slots, open_blockstore, purge_slots, remove_tower, restore_tower,
        run_cluster_partition, run_kill_partition_switch_threshold, test_faulty_node,
        wait_for_last_vote_in_tower_to_land_in_ledger, RUST_LOG_FILTER,
    },
    crossbeam_channel::{unbounded, Receiver},
    gag::BufferRedirect,
    log::*,
    serial_test::serial,
    solana_client::{
        pubsub_client::PubsubClient,
        rpc_client::RpcClient,
        rpc_config::{RpcProgramAccountsConfig, RpcSignatureSubscribeConfig},
        rpc_response::RpcSignatureResult,
        thin_client::{create_client, ThinClient},
    },
    solana_core::{
        broadcast_stage::BroadcastStageType,
        consensus::{Tower, SWITCH_FORK_THRESHOLD, VOTE_THRESHOLD_DEPTH},
        optimistic_confirmation_verifier::OptimisticConfirmationVerifier,
        replay_stage::DUPLICATE_THRESHOLD,
        tower_storage::{FileTowerStorage, SavedTower, SavedTowerVersions, TowerStorage},
        validator::ValidatorConfig,
    },
    solana_download_utils::download_snapshot_archive,
    solana_gossip::{cluster_info::VALIDATOR_PORT_RANGE, gossip_service::discover_cluster},
    solana_ledger::{ancestor_iterator::AncestorIterator, blockstore::Blockstore},
    solana_local_cluster::{
        cluster::{Cluster, ClusterValidatorInfo},
        cluster_tests,
        local_cluster::{ClusterConfig, LocalCluster},
        validator_configs::*,
    },
    solana_runtime::{
        snapshot_archive_info::SnapshotArchiveInfoGetter,
        snapshot_config::SnapshotConfig,
        snapshot_package::SnapshotType,
        snapshot_utils::{self, ArchiveFormat},
    },
    solana_sdk::{
        account::AccountSharedData,
        client::{AsyncClient, SyncClient},
        clock::{self, Slot, DEFAULT_TICKS_PER_SLOT, MAX_PROCESSING_AGE},
        commitment_config::CommitmentConfig,
        epoch_schedule::MINIMUM_SLOTS_PER_EPOCH,
        genesis_config::ClusterType,
        poh_config::PohConfig,
        pubkey::Pubkey,
        signature::{Keypair, Signer},
        system_program, system_transaction,
    },
    solana_streamer::socket::SocketAddrSpace,
    solana_vote_program::vote_state::MAX_LOCKOUT_HISTORY,
    std::{
        collections::{HashMap, HashSet},
        fs,
        io::Read,
        iter,
        path::{Path, PathBuf},
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        thread::{sleep, Builder, JoinHandle},
        time::{Duration, Instant},
    },
    tempfile::TempDir,
};

mod common;

#[test]
fn test_local_cluster_start_and_exit() {
    solana_logger::setup();
    let num_nodes = 1;
    let cluster =
        LocalCluster::new_with_equal_stakes(num_nodes, 100, 3, SocketAddrSpace::Unspecified);
    assert_eq!(cluster.validators.len(), num_nodes);
}

#[test]
fn test_local_cluster_start_and_exit_with_config() {
    solana_logger::setup();
    const NUM_NODES: usize = 1;
    let mut config = ClusterConfig {
        validator_configs: make_identical_validator_configs(
            &ValidatorConfig::default_for_test(),
            NUM_NODES,
        ),
        node_stakes: vec![3; NUM_NODES],
        cluster_lamports: 100,
        ticks_per_slot: 8,
        slots_per_epoch: MINIMUM_SLOTS_PER_EPOCH as u64,
        stakers_slot_offset: MINIMUM_SLOTS_PER_EPOCH as u64,
        ..ClusterConfig::default()
    };
    let cluster = LocalCluster::new(&mut config, SocketAddrSpace::Unspecified);
    assert_eq!(cluster.validators.len(), NUM_NODES);
}

#[test]
#[serial]
fn test_ledger_cleanup_service() {
    solana_logger::setup_with_default(RUST_LOG_FILTER);
    error!("test_ledger_cleanup_service");
    let num_nodes = 3;
    let validator_config = ValidatorConfig {
        max_ledger_shreds: Some(100),
        ..ValidatorConfig::default_for_test()
    };
    let mut config = ClusterConfig {
        cluster_lamports: 10_000,
        poh_config: PohConfig::new_sleep(Duration::from_millis(50)),
        node_stakes: vec![100; num_nodes],
        validator_configs: make_identical_validator_configs(&validator_config, num_nodes),
        ..ClusterConfig::default()
    };
    let mut cluster = LocalCluster::new(&mut config, SocketAddrSpace::Unspecified);
    // 200ms/per * 100 = 20 seconds, so sleep a little longer than that.
    sleep(Duration::from_secs(60));

    cluster_tests::spend_and_verify_all_nodes(
        &cluster.entry_point_info,
        &cluster.funding_keypair,
        num_nodes,
        HashSet::new(),
        SocketAddrSpace::Unspecified,
    );
    cluster.close_preserve_ledgers();
    //check everyone's ledgers and make sure only ~100 slots are stored
    for info in cluster.validators.values() {
        let mut slots = 0;
        let blockstore = Blockstore::open(&info.info.ledger_path).unwrap();
        blockstore
            .slot_meta_iterator(0)
            .unwrap()
            .for_each(|_| slots += 1);
        // with 3 nodes up to 3 slots can be in progress and not complete so max slots in blockstore should be up to 103
        assert!(slots <= 103, "got {}", slots);
    }
}

#[test]
#[serial]
fn test_spend_and_verify_all_nodes_1() {
    solana_logger::setup_with_default(RUST_LOG_FILTER);
    error!("test_spend_and_verify_all_nodes_1");
    let num_nodes = 1;
    let local =
        LocalCluster::new_with_equal_stakes(num_nodes, 10_000, 100, SocketAddrSpace::Unspecified);
    cluster_tests::spend_and_verify_all_nodes(
        &local.entry_point_info,
        &local.funding_keypair,
        num_nodes,
        HashSet::new(),
        SocketAddrSpace::Unspecified,
    );
}

#[test]
#[serial]
fn test_spend_and_verify_all_nodes_2() {
    solana_logger::setup_with_default(RUST_LOG_FILTER);
    error!("test_spend_and_verify_all_nodes_2");
    let num_nodes = 2;
    let local =
        LocalCluster::new_with_equal_stakes(num_nodes, 10_000, 100, SocketAddrSpace::Unspecified);
    cluster_tests::spend_and_verify_all_nodes(
        &local.entry_point_info,
        &local.funding_keypair,
        num_nodes,
        HashSet::new(),
        SocketAddrSpace::Unspecified,
    );
}

#[test]
#[serial]
fn test_spend_and_verify_all_nodes_3() {
    solana_logger::setup_with_default(RUST_LOG_FILTER);
    error!("test_spend_and_verify_all_nodes_3");
    let num_nodes = 3;
    let local =
        LocalCluster::new_with_equal_stakes(num_nodes, 10_000, 100, SocketAddrSpace::Unspecified);
    cluster_tests::spend_and_verify_all_nodes(
        &local.entry_point_info,
        &local.funding_keypair,
        num_nodes,
        HashSet::new(),
        SocketAddrSpace::Unspecified,
    );
}

#[test]
#[serial]
fn test_local_cluster_signature_subscribe() {
    solana_logger::setup_with_default(RUST_LOG_FILTER);
    let num_nodes = 2;
    let cluster =
        LocalCluster::new_with_equal_stakes(num_nodes, 10_000, 100, SocketAddrSpace::Unspecified);
    let nodes = cluster.get_node_pubkeys();

    // Get non leader
    let non_bootstrap_id = nodes
        .into_iter()
        .find(|id| *id != cluster.entry_point_info.id)
        .unwrap();
    let non_bootstrap_info = cluster.get_contact_info(&non_bootstrap_id).unwrap();

    let tx_client = create_client(
        non_bootstrap_info.client_facing_addr(),
        VALIDATOR_PORT_RANGE,
    );
    let (blockhash, _) = tx_client
        .get_latest_blockhash_with_commitment(CommitmentConfig::processed())
        .unwrap();

    let mut transaction = system_transaction::transfer(
        &cluster.funding_keypair,
        &solana_sdk::pubkey::new_rand(),
        10,
        blockhash,
    );

    let (mut sig_subscribe_client, receiver) = PubsubClient::signature_subscribe(
        &format!("ws://{}", &non_bootstrap_info.rpc_pubsub.to_string()),
        &transaction.signatures[0],
        Some(RpcSignatureSubscribeConfig {
            commitment: Some(CommitmentConfig::processed()),
            enable_received_notification: Some(true),
        }),
    )
    .unwrap();

    tx_client
        .retry_transfer(&cluster.funding_keypair, &mut transaction, 5)
        .unwrap();

    let mut got_received_notification = false;
    loop {
        let responses: Vec<_> = receiver.try_iter().collect();
        let mut should_break = false;
        for response in responses {
            match response.value {
                RpcSignatureResult::ProcessedSignature(_) => {
                    should_break = true;
                    break;
                }
                RpcSignatureResult::ReceivedSignature(_) => {
                    got_received_notification = true;
                }
            }
        }

        if should_break {
            break;
        }
        sleep(Duration::from_millis(100));
    }

    // If we don't drop the cluster, the blocking web socket service
    // won't return, and the `sig_subscribe_client` won't shut down
    drop(cluster);
    sig_subscribe_client.shutdown().unwrap();
    assert!(got_received_notification);
}

#[test]
#[allow(unused_attributes)]
#[ignore]
fn test_spend_and_verify_all_nodes_env_num_nodes() {
    solana_logger::setup_with_default(RUST_LOG_FILTER);
    let num_nodes: usize = std::env::var("NUM_NODES")
        .expect("please set environment variable NUM_NODES")
        .parse()
        .expect("could not parse NUM_NODES as a number");
    let local =
        LocalCluster::new_with_equal_stakes(num_nodes, 10_000, 100, SocketAddrSpace::Unspecified);
    cluster_tests::spend_and_verify_all_nodes(
        &local.entry_point_info,
        &local.funding_keypair,
        num_nodes,
        HashSet::new(),
        SocketAddrSpace::Unspecified,
    );
}

// Cluster needs a supermajority to remain, so the minimum size for this test is 4
#[test]
#[serial]
fn test_leader_failure_4() {
    solana_logger::setup_with_default(RUST_LOG_FILTER);
    error!("test_leader_failure_4");
    let num_nodes = 4;
    let validator_config = ValidatorConfig::default_for_test();
    let mut config = ClusterConfig {
        cluster_lamports: 10_000,
        node_stakes: vec![100; 4],
        validator_configs: make_identical_validator_configs(&validator_config, num_nodes),
        ..ClusterConfig::default()
    };
    let local = LocalCluster::new(&mut config, SocketAddrSpace::Unspecified);

    cluster_tests::kill_entry_and_spend_and_verify_rest(
        &local.entry_point_info,
        &local
            .validators
            .get(&local.entry_point_info.id)
            .unwrap()
            .config
            .validator_exit,
        &local.funding_keypair,
        num_nodes,
        config.ticks_per_slot * config.poh_config.target_tick_duration.as_millis() as u64,
        SocketAddrSpace::Unspecified,
    );
}

#[allow(unused_attributes)]
#[ignore]
#[test]
#[serial]
fn test_cluster_partition_1_2() {
    let empty = |_: &mut LocalCluster, _: &mut ()| {};
    let on_partition_resolved = |cluster: &mut LocalCluster, _: &mut ()| {
        cluster.check_for_new_roots(16, "PARTITION_TEST", SocketAddrSpace::Unspecified);
    };
    run_cluster_partition(
        &[vec![1], vec![1, 1]],
        None,
        (),
        empty,
        empty,
        on_partition_resolved,
        None,
        None,
        vec![],
    )
}

#[test]
#[serial]
fn test_cluster_partition_1_1() {
    let empty = |_: &mut LocalCluster, _: &mut ()| {};
    let on_partition_resolved = |cluster: &mut LocalCluster, _: &mut ()| {
        cluster.check_for_new_roots(16, "PARTITION_TEST", SocketAddrSpace::Unspecified);
    };
    run_cluster_partition(
        &[vec![1], vec![1]],
        None,
        (),
        empty,
        empty,
        on_partition_resolved,
        None,
        None,
        vec![],
    )
}

#[test]
#[serial]
fn test_cluster_partition_1_1_1() {
    let empty = |_: &mut LocalCluster, _: &mut ()| {};
    let on_partition_resolved = |cluster: &mut LocalCluster, _: &mut ()| {
        cluster.check_for_new_roots(16, "PARTITION_TEST", SocketAddrSpace::Unspecified);
    };
    run_cluster_partition(
        &[vec![1], vec![1], vec![1]],
        None,
        (),
        empty,
        empty,
        on_partition_resolved,
        None,
        None,
        vec![],
    )
}

#[test]
#[serial]
fn test_two_unbalanced_stakes() {
    solana_logger::setup_with_default(RUST_LOG_FILTER);
    error!("test_two_unbalanced_stakes");
    let validator_config = ValidatorConfig::default_for_test();
    let num_ticks_per_second = 100;
    let num_ticks_per_slot = 10;
    let num_slots_per_epoch = MINIMUM_SLOTS_PER_EPOCH as u64;

    let mut cluster = LocalCluster::new(
        &mut ClusterConfig {
            node_stakes: vec![999_990, 3],
            cluster_lamports: 1_000_000,
            validator_configs: make_identical_validator_configs(&validator_config, 2),
            ticks_per_slot: num_ticks_per_slot,
            slots_per_epoch: num_slots_per_epoch,
            stakers_slot_offset: num_slots_per_epoch,
            poh_config: PohConfig::new_sleep(Duration::from_millis(1000 / num_ticks_per_second)),
            ..ClusterConfig::default()
        },
        SocketAddrSpace::Unspecified,
    );

    cluster_tests::sleep_n_epochs(
        10.0,
        &cluster.genesis_config.poh_config,
        num_ticks_per_slot,
        num_slots_per_epoch,
    );
    cluster.close_preserve_ledgers();
    let leader_pubkey = cluster.entry_point_info.id;
    let leader_ledger = cluster.validators[&leader_pubkey].info.ledger_path.clone();
    cluster_tests::verify_ledger_ticks(&leader_ledger, num_ticks_per_slot as usize);
}

#[test]
#[serial]
fn test_forwarding() {
    // Set up a cluster where one node is never the leader, so all txs sent to this node
    // will be have to be forwarded in order to be confirmed
    let mut config = ClusterConfig {
        node_stakes: vec![999_990, 3],
        cluster_lamports: 2_000_000,
        validator_configs: make_identical_validator_configs(
            &ValidatorConfig::default_for_test(),
            2,
        ),
        ..ClusterConfig::default()
    };
    let cluster = LocalCluster::new(&mut config, SocketAddrSpace::Unspecified);

    let cluster_nodes = discover_cluster(
        &cluster.entry_point_info.gossip,
        2,
        SocketAddrSpace::Unspecified,
    )
    .unwrap();
    assert!(cluster_nodes.len() >= 2);

    let leader_pubkey = cluster.entry_point_info.id;

    let validator_info = cluster_nodes
        .iter()
        .find(|c| c.id != leader_pubkey)
        .unwrap();

    // Confirm that transactions were forwarded to and processed by the leader.
    cluster_tests::send_many_transactions(validator_info, &cluster.funding_keypair, 10, 20);
}

#[test]
#[serial]
fn test_restart_node() {
    solana_logger::setup_with_default(RUST_LOG_FILTER);
    error!("test_restart_node");
    let slots_per_epoch = MINIMUM_SLOTS_PER_EPOCH * 2;
    let ticks_per_slot = 16;
    let validator_config = ValidatorConfig::default_for_test();
    let mut cluster = LocalCluster::new(
        &mut ClusterConfig {
            node_stakes: vec![100; 1],
            cluster_lamports: 100,
            validator_configs: vec![safe_clone_config(&validator_config)],
            ticks_per_slot,
            slots_per_epoch,
            stakers_slot_offset: slots_per_epoch,
            ..ClusterConfig::default()
        },
        SocketAddrSpace::Unspecified,
    );
    let nodes = cluster.get_node_pubkeys();
    cluster_tests::sleep_n_epochs(
        1.0,
        &cluster.genesis_config.poh_config,
        clock::DEFAULT_TICKS_PER_SLOT,
        slots_per_epoch,
    );
    cluster.exit_restart_node(&nodes[0], validator_config, SocketAddrSpace::Unspecified);
    cluster_tests::sleep_n_epochs(
        0.5,
        &cluster.genesis_config.poh_config,
        clock::DEFAULT_TICKS_PER_SLOT,
        slots_per_epoch,
    );
    cluster_tests::send_many_transactions(
        &cluster.entry_point_info,
        &cluster.funding_keypair,
        10,
        1,
    );
}

#[test]
#[serial]
fn test_mainnet_beta_cluster_type() {
    solana_logger::setup_with_default(RUST_LOG_FILTER);

    let mut config = ClusterConfig {
        cluster_type: ClusterType::MainnetBeta,
        node_stakes: vec![100; 1],
        cluster_lamports: 1_000,
        validator_configs: make_identical_validator_configs(
            &ValidatorConfig::default_for_test(),
            1,
        ),
        ..ClusterConfig::default()
    };
    let cluster = LocalCluster::new(&mut config, SocketAddrSpace::Unspecified);
    let cluster_nodes = discover_cluster(
        &cluster.entry_point_info.gossip,
        1,
        SocketAddrSpace::Unspecified,
    )
    .unwrap();
    assert_eq!(cluster_nodes.len(), 1);

    let client = create_client(
        cluster.entry_point_info.client_facing_addr(),
        VALIDATOR_PORT_RANGE,
    );

    // Programs that are available at epoch 0
    for program_id in [
        &solana_config_program::id(),
        &solana_sdk::system_program::id(),
        &solana_sdk::stake::program::id(),
        &solana_vote_program::id(),
        &solana_sdk::bpf_loader_deprecated::id(),
        &solana_sdk::bpf_loader::id(),
        &solana_sdk::bpf_loader_upgradeable::id(),
    ]
    .iter()
    {
        assert_matches!(
            (
                program_id,
                client
                    .get_account_with_commitment(program_id, CommitmentConfig::processed())
                    .unwrap()
            ),
            (_program_id, Some(_))
        );
    }

    // Programs that are not available at epoch 0
    for program_id in [].iter() {
        assert_eq!(
            (
                program_id,
                client
                    .get_account_with_commitment(program_id, CommitmentConfig::processed())
                    .unwrap()
            ),
            (program_id, None)
        );
    }
}

#[test]
#[serial]
fn test_consistency_halt() {
    solana_logger::setup_with_default(RUST_LOG_FILTER);
    let snapshot_interval_slots = 20;
    let num_account_paths = 1;

    // Create cluster with a leader producing bad snapshot hashes.
    let mut leader_snapshot_test_config =
        setup_snapshot_validator_config(snapshot_interval_slots, num_account_paths);
    leader_snapshot_test_config
        .validator_config
        .accounts_hash_fault_injection_slots = 40;

    let validator_stake = 10_000;
    let mut config = ClusterConfig {
        node_stakes: vec![validator_stake],
        cluster_lamports: 100_000,
        validator_configs: vec![leader_snapshot_test_config.validator_config],
        ..ClusterConfig::default()
    };

    let mut cluster = LocalCluster::new(&mut config, SocketAddrSpace::Unspecified);

    sleep(Duration::from_millis(5000));
    let cluster_nodes = discover_cluster(
        &cluster.entry_point_info.gossip,
        1,
        SocketAddrSpace::Unspecified,
    )
    .unwrap();
    info!("num_nodes: {}", cluster_nodes.len());

    // Add a validator with the leader as trusted, it should halt when it detects
    // mismatch.
    let mut validator_snapshot_test_config =
        setup_snapshot_validator_config(snapshot_interval_slots, num_account_paths);

    let mut known_validators = HashSet::new();
    known_validators.insert(cluster_nodes[0].id);

    validator_snapshot_test_config
        .validator_config
        .known_validators = Some(known_validators);
    validator_snapshot_test_config
        .validator_config
        .halt_on_known_validators_accounts_hash_mismatch = true;

    warn!("adding a validator");
    cluster.add_validator(
        &validator_snapshot_test_config.validator_config,
        validator_stake as u64,
        Arc::new(Keypair::new()),
        None,
        SocketAddrSpace::Unspecified,
    );
    let num_nodes = 2;
    assert_eq!(
        discover_cluster(
            &cluster.entry_point_info.gossip,
            num_nodes,
            SocketAddrSpace::Unspecified
        )
        .unwrap()
        .len(),
        num_nodes
    );

    // Check for only 1 node on the network.
    let mut encountered_error = false;
    loop {
        let discover = discover_cluster(
            &cluster.entry_point_info.gossip,
            2,
            SocketAddrSpace::Unspecified,
        );
        match discover {
            Err(_) => {
                encountered_error = true;
                break;
            }
            Ok(nodes) => {
                if nodes.len() < 2 {
                    encountered_error = true;
                    break;
                }
                info!("checking cluster for fewer nodes.. {:?}", nodes.len());
            }
        }
        let client = cluster
            .get_validator_client(&cluster.entry_point_info.id)
            .unwrap();
        if let Ok(slot) = client.get_slot() {
            if slot > 210 {
                break;
            }
            info!("slot: {}", slot);
        }
        sleep(Duration::from_millis(1000));
    }
    assert!(encountered_error);
}

#[test]
#[serial]
fn test_snapshot_download() {
    solana_logger::setup_with_default(RUST_LOG_FILTER);
    // First set up the cluster with 1 node
    let snapshot_interval_slots = 50;
    let num_account_paths = 3;

    let leader_snapshot_test_config =
        setup_snapshot_validator_config(snapshot_interval_slots, num_account_paths);
    let validator_snapshot_test_config =
        setup_snapshot_validator_config(snapshot_interval_slots, num_account_paths);

    let stake = 10_000;
    let mut config = ClusterConfig {
        node_stakes: vec![stake],
        cluster_lamports: 1_000_000,
        validator_configs: make_identical_validator_configs(
            &leader_snapshot_test_config.validator_config,
            1,
        ),
        ..ClusterConfig::default()
    };

    let mut cluster = LocalCluster::new(&mut config, SocketAddrSpace::Unspecified);

    let snapshot_archives_dir = &leader_snapshot_test_config
        .validator_config
        .snapshot_config
        .as_ref()
        .unwrap()
        .snapshot_archives_dir;

    trace!("Waiting for snapshot");
    let full_snapshot_archive_info = cluster.wait_for_next_full_snapshot(snapshot_archives_dir);
    trace!("found: {}", full_snapshot_archive_info.path().display());

    // Download the snapshot, then boot a validator from it.
    download_snapshot_archive(
        &cluster.entry_point_info.rpc,
        snapshot_archives_dir,
        (
            full_snapshot_archive_info.slot(),
            *full_snapshot_archive_info.hash(),
        ),
        SnapshotType::FullSnapshot,
        validator_snapshot_test_config
            .validator_config
            .snapshot_config
            .as_ref()
            .unwrap()
            .maximum_full_snapshot_archives_to_retain,
        validator_snapshot_test_config
            .validator_config
            .snapshot_config
            .as_ref()
            .unwrap()
            .maximum_incremental_snapshot_archives_to_retain,
        false,
        &mut None,
    )
    .unwrap();

    cluster.add_validator(
        &validator_snapshot_test_config.validator_config,
        stake,
        Arc::new(Keypair::new()),
        None,
        SocketAddrSpace::Unspecified,
    );
}

#[test]
#[serial]
fn test_incremental_snapshot_download() {
    solana_logger::setup_with_default(RUST_LOG_FILTER);
    // First set up the cluster with 1 node
    let accounts_hash_interval = 3;
    let incremental_snapshot_interval = accounts_hash_interval * 3;
    let full_snapshot_interval = incremental_snapshot_interval * 3;
    let num_account_paths = 3;

    let leader_snapshot_test_config = SnapshotValidatorConfig::new(
        full_snapshot_interval,
        incremental_snapshot_interval,
        accounts_hash_interval,
        num_account_paths,
    );
    let validator_snapshot_test_config = SnapshotValidatorConfig::new(
        full_snapshot_interval,
        incremental_snapshot_interval,
        accounts_hash_interval,
        num_account_paths,
    );

    let stake = 10_000;
    let mut config = ClusterConfig {
        node_stakes: vec![stake],
        cluster_lamports: 1_000_000,
        validator_configs: make_identical_validator_configs(
            &leader_snapshot_test_config.validator_config,
            1,
        ),
        ..ClusterConfig::default()
    };

    let mut cluster = LocalCluster::new(&mut config, SocketAddrSpace::Unspecified);

    let snapshot_archives_dir = &leader_snapshot_test_config
        .validator_config
        .snapshot_config
        .as_ref()
        .unwrap()
        .snapshot_archives_dir;

    debug!("snapshot config:\n\tfull snapshot interval: {}\n\tincremental snapshot interval: {}\n\taccounts hash interval: {}",
           full_snapshot_interval,
           incremental_snapshot_interval,
           accounts_hash_interval);
    debug!(
        "leader config:\n\tbank snapshots dir: {}\n\tsnapshot archives dir: {}",
        leader_snapshot_test_config
            .bank_snapshots_dir
            .path()
            .display(),
        leader_snapshot_test_config
            .snapshot_archives_dir
            .path()
            .display(),
    );
    debug!(
        "validator config:\n\tbank snapshots dir: {}\n\tsnapshot archives dir: {}",
        validator_snapshot_test_config
            .bank_snapshots_dir
            .path()
            .display(),
        validator_snapshot_test_config
            .snapshot_archives_dir
            .path()
            .display(),
    );

    trace!("Waiting for snapshots");
    let (incremental_snapshot_archive_info, full_snapshot_archive_info) =
        cluster.wait_for_next_incremental_snapshot(snapshot_archives_dir);
    trace!(
        "found: {} and {}",
        full_snapshot_archive_info.path().display(),
        incremental_snapshot_archive_info.path().display()
    );

    // Download the snapshots, then boot a validator from them.
    download_snapshot_archive(
        &cluster.entry_point_info.rpc,
        snapshot_archives_dir,
        (
            full_snapshot_archive_info.slot(),
            *full_snapshot_archive_info.hash(),
        ),
        SnapshotType::FullSnapshot,
        validator_snapshot_test_config
            .validator_config
            .snapshot_config
            .as_ref()
            .unwrap()
            .maximum_full_snapshot_archives_to_retain,
        validator_snapshot_test_config
            .validator_config
            .snapshot_config
            .as_ref()
            .unwrap()
            .maximum_incremental_snapshot_archives_to_retain,
        false,
        &mut None,
    )
    .unwrap();

    download_snapshot_archive(
        &cluster.entry_point_info.rpc,
        snapshot_archives_dir,
        (
            incremental_snapshot_archive_info.slot(),
            *incremental_snapshot_archive_info.hash(),
        ),
        SnapshotType::IncrementalSnapshot(incremental_snapshot_archive_info.base_slot()),
        validator_snapshot_test_config
            .validator_config
            .snapshot_config
            .as_ref()
            .unwrap()
            .maximum_full_snapshot_archives_to_retain,
        validator_snapshot_test_config
            .validator_config
            .snapshot_config
            .as_ref()
            .unwrap()
            .maximum_incremental_snapshot_archives_to_retain,
        false,
        &mut None,
    )
    .unwrap();

    cluster.add_validator(
        &validator_snapshot_test_config.validator_config,
        stake,
        Arc::new(Keypair::new()),
        None,
        SocketAddrSpace::Unspecified,
    );
}

/// Test the scenario where a node starts up from a snapshot and its blockstore has enough new
/// roots that cross the full snapshot interval.  In this scenario, the node needs to take a full
/// snapshot while processing the blockstore so that once the background services start up, there
/// is the correct full snapshot available to take subsequent incremental snapshots.
///
/// For this test...
/// - Start a leader node and run it long enough to take a full and incremental snapshot
/// - Download those snapshots to a validator node
/// - Copy the validator snapshots to a back up directory
/// - Start up the validator node
/// - Wait for the validator node to see enough root slots to cross the full snapshot interval
/// - Delete the snapshots on the validator node and restore the ones from the backup
/// - Restart the validator node to trigger the scenario we're trying to test
/// - Wait for the validator node to generate a new incremental snapshot
/// - Copy the new incremental snapshot (and its associated full snapshot) to another new validator
/// - Start up this new validator to ensure the snapshots from ^^^ are good
#[test]
#[serial]
fn test_incremental_snapshot_download_with_crossing_full_snapshot_interval_at_startup() {
    solana_logger::setup_with_default(RUST_LOG_FILTER);
    // If these intervals change, also make sure to change the loop timers accordingly.
    let accounts_hash_interval = 3;
    let incremental_snapshot_interval = accounts_hash_interval * 3;
    let full_snapshot_interval = incremental_snapshot_interval * 3;

    let num_account_paths = 3;
    let leader_snapshot_test_config = SnapshotValidatorConfig::new(
        full_snapshot_interval,
        incremental_snapshot_interval,
        accounts_hash_interval,
        num_account_paths,
    );
    let validator_snapshot_test_config = SnapshotValidatorConfig::new(
        full_snapshot_interval,
        incremental_snapshot_interval,
        accounts_hash_interval,
        num_account_paths,
    );
    let stake = 10_000;
    let mut config = ClusterConfig {
        node_stakes: vec![stake],
        cluster_lamports: 1_000_000,
        validator_configs: make_identical_validator_configs(
            &leader_snapshot_test_config.validator_config,
            1,
        ),
        ..ClusterConfig::default()
    };

    let mut cluster = LocalCluster::new(&mut config, SocketAddrSpace::Unspecified);

    debug!("snapshot config:\n\tfull snapshot interval: {}\n\tincremental snapshot interval: {}\n\taccounts hash interval: {}",
           full_snapshot_interval,
           incremental_snapshot_interval,
           accounts_hash_interval);
    debug!(
        "leader config:\n\tbank snapshots dir: {}\n\tsnapshot archives dir: {}",
        leader_snapshot_test_config
            .bank_snapshots_dir
            .path()
            .display(),
        leader_snapshot_test_config
            .snapshot_archives_dir
            .path()
            .display(),
    );
    debug!(
        "validator config:\n\tbank snapshots dir: {}\n\tsnapshot archives dir: {}",
        validator_snapshot_test_config
            .bank_snapshots_dir
            .path()
            .display(),
        validator_snapshot_test_config
            .snapshot_archives_dir
            .path()
            .display(),
    );

    info!("Waiting for leader to create snapshots...");
    let (incremental_snapshot_archive_info, full_snapshot_archive_info) =
        LocalCluster::wait_for_next_incremental_snapshot(
            &cluster,
            leader_snapshot_test_config.snapshot_archives_dir.path(),
        );
    debug!(
        "Found snapshots:\n\tfull snapshot: {}\n\tincremental snapshot: {}",
        full_snapshot_archive_info.path().display(),
        incremental_snapshot_archive_info.path().display()
    );
    assert_eq!(
        full_snapshot_archive_info.slot(),
        incremental_snapshot_archive_info.base_slot()
    );

    // Download the snapshots, then boot a validator from them.
    info!("Downloading full snapshot to validator...");
    download_snapshot_archive(
        &cluster.entry_point_info.rpc,
        validator_snapshot_test_config.snapshot_archives_dir.path(),
        (
            full_snapshot_archive_info.slot(),
            *full_snapshot_archive_info.hash(),
        ),
        SnapshotType::FullSnapshot,
        validator_snapshot_test_config
            .validator_config
            .snapshot_config
            .as_ref()
            .unwrap()
            .maximum_full_snapshot_archives_to_retain,
        validator_snapshot_test_config
            .validator_config
            .snapshot_config
            .as_ref()
            .unwrap()
            .maximum_incremental_snapshot_archives_to_retain,
        false,
        &mut None,
    )
    .unwrap();
    let downloaded_full_snapshot_archive_info =
        snapshot_utils::get_highest_full_snapshot_archive_info(
            validator_snapshot_test_config.snapshot_archives_dir.path(),
        )
        .unwrap();
    debug!(
        "Downloaded full snapshot, slot: {}",
        downloaded_full_snapshot_archive_info.slot()
    );

    info!("Downloading incremental snapshot to validator...");
    download_snapshot_archive(
        &cluster.entry_point_info.rpc,
        validator_snapshot_test_config.snapshot_archives_dir.path(),
        (
            incremental_snapshot_archive_info.slot(),
            *incremental_snapshot_archive_info.hash(),
        ),
        SnapshotType::IncrementalSnapshot(incremental_snapshot_archive_info.base_slot()),
        validator_snapshot_test_config
            .validator_config
            .snapshot_config
            .as_ref()
            .unwrap()
            .maximum_full_snapshot_archives_to_retain,
        validator_snapshot_test_config
            .validator_config
            .snapshot_config
            .as_ref()
            .unwrap()
            .maximum_incremental_snapshot_archives_to_retain,
        false,
        &mut None,
    )
    .unwrap();
    let downloaded_incremental_snapshot_archive_info =
        snapshot_utils::get_highest_incremental_snapshot_archive_info(
            validator_snapshot_test_config.snapshot_archives_dir.path(),
            full_snapshot_archive_info.slot(),
        )
        .unwrap();
    debug!(
        "Downloaded incremental snapshot, slot: {}, base slot: {}",
        downloaded_incremental_snapshot_archive_info.slot(),
        downloaded_incremental_snapshot_archive_info.base_slot(),
    );
    assert_eq!(
        downloaded_full_snapshot_archive_info.slot(),
        downloaded_incremental_snapshot_archive_info.base_slot()
    );

    // closure to copy files in a directory to another directory
    let copy_files = |from: &Path, to: &Path| {
        trace!(
            "copying files from dir {}, to dir {}",
            from.display(),
            to.display()
        );
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                continue;
            }
            let from_file_path = entry.path();
            let to_file_path = to.join(from_file_path.file_name().unwrap());
            trace!(
                "\t\tcopying file from {} to {}...",
                from_file_path.display(),
                to_file_path.display()
            );
            fs::copy(from_file_path, to_file_path).unwrap();
        }
    };
    // closure to delete files in a directory
    let delete_files = |dir: &Path| {
        trace!("deleting files in dir {}", dir.display());
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                continue;
            }
            let file_path = entry.path();
            trace!("\t\tdeleting file {}...", file_path.display());
            fs::remove_file(file_path).unwrap();
        }
    };

    // After downloading the snapshots, copy them over to a backup directory.  Later we'll need to
    // restart the node and guarantee that the only snapshots present are these initial ones.  So,
    // the easiest way to do that is create a backup now, delete the ones on the node before
    // restart, then copy the backup ones over again.
    let backup_validator_snapshot_archives_dir = tempfile::tempdir_in(farf_dir()).unwrap();
    trace!(
        "Backing up validator snapshots to dir: {}...",
        backup_validator_snapshot_archives_dir.path().display()
    );
    copy_files(
        validator_snapshot_test_config.snapshot_archives_dir.path(),
        backup_validator_snapshot_archives_dir.path(),
    );

    info!("Starting a new validator...");
    let validator_identity = Arc::new(Keypair::new());
    cluster.add_validator(
        &validator_snapshot_test_config.validator_config,
        stake,
        validator_identity.clone(),
        None,
        SocketAddrSpace::Unspecified,
    );

    // To ensure that a snapshot will be taken during startup, the blockstore needs to have roots
    // that cross a full snapshot interval.
    info!("Waiting for the validator to see enough slots to cross a full snapshot interval...");
    let starting_slot = incremental_snapshot_archive_info.slot();
    let timer = Instant::now();
    loop {
        let validator_current_slot = cluster
            .get_validator_client(&validator_identity.pubkey())
            .unwrap()
            .get_slot_with_commitment(CommitmentConfig::finalized())
            .unwrap();
        if validator_current_slot > (starting_slot + full_snapshot_interval) {
            break;
        }
        assert!(
            timer.elapsed() < Duration::from_secs(30),
            "It should not take longer than 30 seconds to cross the next full snapshot interval."
        );
        std::thread::yield_now();
    }
    trace!("Waited {:?}", timer.elapsed());

    // Get the highest full snapshot archive info for the validator, now that it has crossed the
    // next full snapshot interval.  We are going to use this to look up the same snapshot on the
    // leader, which we'll then use to compare to the full snapshot the validator will create
    // during startup.  This ensures the snapshot creation process during startup is correct.
    //
    // Putting this all in its own block so its clear we're only intended to keep the leader's info
    let leader_full_snapshot_archive_info_for_comparison = {
        let validator_full_snapshot = snapshot_utils::get_highest_full_snapshot_archive_info(
            validator_snapshot_test_config.snapshot_archives_dir.path(),
        )
        .unwrap();

        // Now get the same full snapshot on the LEADER that we just got from the validator
        let mut leader_full_snapshots = snapshot_utils::get_full_snapshot_archives(
            leader_snapshot_test_config.snapshot_archives_dir.path(),
        );
        leader_full_snapshots.retain(|full_snapshot| {
            full_snapshot.slot() == validator_full_snapshot.slot()
                && full_snapshot.hash() == validator_full_snapshot.hash()
        });

        // NOTE: If this unwrap() ever fails, it may be that the leader's old full snapshot archives
        // were purged.  If that happens, increase the maximum_full_snapshot_archives_to_retain
        // in the leader's Snapshotconfig.
        let leader_full_snapshot = leader_full_snapshots.first().unwrap();

        // And for sanity, the full snapshot from the leader and the validator MUST be the same
        assert_eq!(
            (
                validator_full_snapshot.slot(),
                validator_full_snapshot.hash()
            ),
            (leader_full_snapshot.slot(), leader_full_snapshot.hash())
        );

        leader_full_snapshot.clone()
    };

    trace!(
        "Delete all the snapshots on the validator and restore the originals from the backup..."
    );
    delete_files(validator_snapshot_test_config.snapshot_archives_dir.path());
    copy_files(
        backup_validator_snapshot_archives_dir.path(),
        validator_snapshot_test_config.snapshot_archives_dir.path(),
    );

    // Get the highest full snapshot slot *before* restarting, as a comparison
    let validator_full_snapshot_slot_at_startup =
        snapshot_utils::get_highest_full_snapshot_archive_slot(
            validator_snapshot_test_config.snapshot_archives_dir.path(),
        )
        .unwrap();

    info!("Restarting the validator...");
    let validator_info = cluster.exit_node(&validator_identity.pubkey());
    cluster.restart_node(
        &validator_identity.pubkey(),
        validator_info,
        SocketAddrSpace::Unspecified,
    );

    // Now, we want to ensure that the validator can make a new incremental snapshot based on the
    // new full snapshot that was created during the restart.
    let timer = Instant::now();
    let (
        validator_highest_full_snapshot_archive_info,
        _validator_highest_incremental_snapshot_archive_info,
    ) = loop {
        if let Some(highest_full_snapshot_info) =
            snapshot_utils::get_highest_full_snapshot_archive_info(
                validator_snapshot_test_config.snapshot_archives_dir.path(),
            )
        {
            if highest_full_snapshot_info.slot() > validator_full_snapshot_slot_at_startup {
                if let Some(highest_incremental_snapshot_info) =
                    snapshot_utils::get_highest_incremental_snapshot_archive_info(
                        validator_snapshot_test_config.snapshot_archives_dir.path(),
                        highest_full_snapshot_info.slot(),
                    )
                {
                    info!("Success! Made new full and incremental snapshots!");
                    trace!(
                        "Full snapshot slot: {}, incremental snapshot slot: {}",
                        highest_full_snapshot_info.slot(),
                        highest_incremental_snapshot_info.slot(),
                    );
                    break (
                        highest_full_snapshot_info,
                        highest_incremental_snapshot_info,
                    );
                }
            }
        }
        assert!(
            timer.elapsed() < Duration::from_secs(10),
            "It should not take longer than 10 seconds to cross the next incremental snapshot interval."
        );
        std::thread::yield_now();
    };
    trace!("Waited {:?}", timer.elapsed());

    // Check to make sure that the full snapshot the validator created during startup is the same
    // as the snapshot the leader created.
    // NOTE: If the assert fires and the _slots_ don't match (specifically are off by a full
    // snapshot interval), then that means the loop to get the
    // `validator_highest_full_snapshot_archive_info` saw the wrong one, and that may've been due
    // to some weird scheduling/delays on the machine running the test.  Run the test again.  If
    // this ever fails repeatedly then the test will need to be modified to handle this case.
    assert_eq!(
        (
            validator_highest_full_snapshot_archive_info.slot(),
            validator_highest_full_snapshot_archive_info.hash()
        ),
        (
            leader_full_snapshot_archive_info_for_comparison.slot(),
            leader_full_snapshot_archive_info_for_comparison.hash()
        )
    );

    // And lastly, startup another node with the new snapshots to ensure they work
    let final_validator_snapshot_test_config = SnapshotValidatorConfig::new(
        full_snapshot_interval,
        incremental_snapshot_interval,
        accounts_hash_interval,
        num_account_paths,
    );

    // Copy over the snapshots to the new node, but need to remove the tmp snapshot dir so it
    // doesn't break the simple copy_files closure.
    snapshot_utils::remove_tmp_snapshot_archives(
        validator_snapshot_test_config.snapshot_archives_dir.path(),
    );
    copy_files(
        validator_snapshot_test_config.snapshot_archives_dir.path(),
        final_validator_snapshot_test_config
            .snapshot_archives_dir
            .path(),
    );

    info!("Starting final validator...");
    let final_validator_identity = Arc::new(Keypair::new());
    cluster.add_validator(
        &final_validator_snapshot_test_config.validator_config,
        stake,
        final_validator_identity,
        None,
        SocketAddrSpace::Unspecified,
    );

    // Success!
}

#[allow(unused_attributes)]
#[test]
#[serial]
fn test_snapshot_restart_tower() {
    solana_logger::setup_with_default(RUST_LOG_FILTER);
    // First set up the cluster with 2 nodes
    let snapshot_interval_slots = 10;
    let num_account_paths = 2;

    let leader_snapshot_test_config =
        setup_snapshot_validator_config(snapshot_interval_slots, num_account_paths);
    let validator_snapshot_test_config =
        setup_snapshot_validator_config(snapshot_interval_slots, num_account_paths);

    let mut config = ClusterConfig {
        node_stakes: vec![10000, 10],
        cluster_lamports: 100_000,
        validator_configs: vec![
            safe_clone_config(&leader_snapshot_test_config.validator_config),
            safe_clone_config(&validator_snapshot_test_config.validator_config),
        ],
        ..ClusterConfig::default()
    };

    let mut cluster = LocalCluster::new(&mut config, SocketAddrSpace::Unspecified);

    // Let the nodes run for a while, then stop one of the validators
    sleep(Duration::from_millis(5000));
    let all_pubkeys = cluster.get_node_pubkeys();
    let validator_id = all_pubkeys
        .into_iter()
        .find(|x| *x != cluster.entry_point_info.id)
        .unwrap();
    let validator_info = cluster.exit_node(&validator_id);

    // Get slot after which this was generated
    let snapshot_archives_dir = &leader_snapshot_test_config
        .validator_config
        .snapshot_config
        .as_ref()
        .unwrap()
        .snapshot_archives_dir;

    let full_snapshot_archive_info = cluster.wait_for_next_full_snapshot(snapshot_archives_dir);

    // Copy archive to validator's snapshot output directory
    let validator_archive_path = snapshot_utils::build_full_snapshot_archive_path(
        validator_snapshot_test_config
            .snapshot_archives_dir
            .path()
            .to_path_buf(),
        full_snapshot_archive_info.slot(),
        full_snapshot_archive_info.hash(),
        full_snapshot_archive_info.archive_format(),
    );
    fs::hard_link(full_snapshot_archive_info.path(), &validator_archive_path).unwrap();

    // Restart validator from snapshot, the validator's tower state in this snapshot
    // will contain slots < the root bank of the snapshot. Validator should not panic.
    cluster.restart_node(&validator_id, validator_info, SocketAddrSpace::Unspecified);

    // Test cluster can still make progress and get confirmations in tower
    // Use the restarted node as the discovery point so that we get updated
    // validator's ContactInfo
    let restarted_node_info = cluster.get_contact_info(&validator_id).unwrap();
    cluster_tests::spend_and_verify_all_nodes(
        restarted_node_info,
        &cluster.funding_keypair,
        1,
        HashSet::new(),
        SocketAddrSpace::Unspecified,
    );
}

#[test]
#[serial]
fn test_snapshots_blockstore_floor() {
    solana_logger::setup_with_default(RUST_LOG_FILTER);
    // First set up the cluster with 1 snapshotting leader
    let snapshot_interval_slots = 10;
    let num_account_paths = 4;

    let leader_snapshot_test_config =
        setup_snapshot_validator_config(snapshot_interval_slots, num_account_paths);
    let mut validator_snapshot_test_config =
        setup_snapshot_validator_config(snapshot_interval_slots, num_account_paths);

    let snapshot_archives_dir = &leader_snapshot_test_config
        .validator_config
        .snapshot_config
        .as_ref()
        .unwrap()
        .snapshot_archives_dir;

    let mut config = ClusterConfig {
        node_stakes: vec![10000],
        cluster_lamports: 100_000,
        validator_configs: make_identical_validator_configs(
            &leader_snapshot_test_config.validator_config,
            1,
        ),
        ..ClusterConfig::default()
    };

    let mut cluster = LocalCluster::new(&mut config, SocketAddrSpace::Unspecified);

    trace!("Waiting for snapshot tar to be generated with slot",);

    let archive_info = loop {
        let archive =
            snapshot_utils::get_highest_full_snapshot_archive_info(&snapshot_archives_dir);
        if archive.is_some() {
            trace!("snapshot exists");
            break archive.unwrap();
        }
        sleep(Duration::from_millis(5000));
    };

    // Copy archive to validator's snapshot output directory
    let validator_archive_path = snapshot_utils::build_full_snapshot_archive_path(
        validator_snapshot_test_config
            .snapshot_archives_dir
            .path()
            .to_path_buf(),
        archive_info.slot(),
        archive_info.hash(),
        ArchiveFormat::TarBzip2,
    );
    fs::hard_link(archive_info.path(), &validator_archive_path).unwrap();
    let slot_floor = archive_info.slot();

    // Start up a new node from a snapshot
    let validator_stake = 5;

    let cluster_nodes = discover_cluster(
        &cluster.entry_point_info.gossip,
        1,
        SocketAddrSpace::Unspecified,
    )
    .unwrap();
    let mut known_validators = HashSet::new();
    known_validators.insert(cluster_nodes[0].id);
    validator_snapshot_test_config
        .validator_config
        .known_validators = Some(known_validators);

    cluster.add_validator(
        &validator_snapshot_test_config.validator_config,
        validator_stake,
        Arc::new(Keypair::new()),
        None,
        SocketAddrSpace::Unspecified,
    );
    let all_pubkeys = cluster.get_node_pubkeys();
    let validator_id = all_pubkeys
        .into_iter()
        .find(|x| *x != cluster.entry_point_info.id)
        .unwrap();
    let validator_client = cluster.get_validator_client(&validator_id).unwrap();
    let mut current_slot = 0;

    // Let this validator run a while with repair
    let target_slot = slot_floor + 40;
    while current_slot <= target_slot {
        trace!("current_slot: {}", current_slot);
        if let Ok(slot) = validator_client.get_slot_with_commitment(CommitmentConfig::processed()) {
            current_slot = slot;
        } else {
            continue;
        }
        sleep(Duration::from_secs(1));
    }

    // Check the validator ledger doesn't contain any slots < slot_floor
    cluster.close_preserve_ledgers();
    let validator_ledger_path = &cluster.validators[&validator_id];
    let blockstore = Blockstore::open(&validator_ledger_path.info.ledger_path).unwrap();

    // Skip the zeroth slot in blockstore that the ledger is initialized with
    let (first_slot, _) = blockstore.slot_meta_iterator(1).unwrap().next().unwrap();

    assert_eq!(first_slot, slot_floor);
}

#[test]
#[serial]
fn test_snapshots_restart_validity() {
    solana_logger::setup_with_default(RUST_LOG_FILTER);
    let snapshot_interval_slots = 10;
    let num_account_paths = 1;
    let mut snapshot_test_config =
        setup_snapshot_validator_config(snapshot_interval_slots, num_account_paths);
    let snapshot_archives_dir = &snapshot_test_config
        .validator_config
        .snapshot_config
        .as_ref()
        .unwrap()
        .snapshot_archives_dir;

    // Set up the cluster with 1 snapshotting validator
    let mut all_account_storage_dirs = vec![vec![]];

    std::mem::swap(
        &mut all_account_storage_dirs[0],
        &mut snapshot_test_config.account_storage_dirs,
    );

    let mut config = ClusterConfig {
        node_stakes: vec![10000],
        cluster_lamports: 100_000,
        validator_configs: make_identical_validator_configs(
            &snapshot_test_config.validator_config,
            1,
        ),
        ..ClusterConfig::default()
    };

    // Create and reboot the node from snapshot `num_runs` times
    let num_runs = 3;
    let mut expected_balances = HashMap::new();
    let mut cluster = LocalCluster::new(&mut config, SocketAddrSpace::Unspecified);
    for i in 1..num_runs {
        info!("run {}", i);
        // Push transactions to one of the nodes and confirm that transactions were
        // forwarded to and processed.
        trace!("Sending transactions");
        let new_balances = cluster_tests::send_many_transactions(
            &cluster.entry_point_info,
            &cluster.funding_keypair,
            10,
            10,
        );

        expected_balances.extend(new_balances);

        cluster.wait_for_next_full_snapshot(snapshot_archives_dir);

        // Create new account paths since validator exit is not guaranteed to cleanup RPC threads,
        // which may delete the old accounts on exit at any point
        let (new_account_storage_dirs, new_account_storage_paths) =
            generate_account_paths(num_account_paths);
        all_account_storage_dirs.push(new_account_storage_dirs);
        snapshot_test_config.validator_config.account_paths = new_account_storage_paths;

        // Restart node
        trace!("Restarting cluster from snapshot");
        let nodes = cluster.get_node_pubkeys();
        cluster.exit_restart_node(
            &nodes[0],
            safe_clone_config(&snapshot_test_config.validator_config),
            SocketAddrSpace::Unspecified,
        );

        // Verify account balances on validator
        trace!("Verifying balances");
        cluster_tests::verify_balances(expected_balances.clone(), &cluster.entry_point_info);

        // Check that we can still push transactions
        trace!("Spending and verifying");
        cluster_tests::spend_and_verify_all_nodes(
            &cluster.entry_point_info,
            &cluster.funding_keypair,
            1,
            HashSet::new(),
            SocketAddrSpace::Unspecified,
        );
    }
}

#[test]
#[serial]
#[allow(unused_attributes)]
#[ignore]
fn test_fail_entry_verification_leader() {
    let leader_stake = (DUPLICATE_THRESHOLD * 100.0) as u64 + 1;
    let validator_stake1 = (100 - leader_stake) / 2;
    let validator_stake2 = 100 - leader_stake - validator_stake1;
    let (cluster, _) = test_faulty_node(
        BroadcastStageType::FailEntryVerification,
        vec![leader_stake, validator_stake1, validator_stake2],
    );
    cluster.check_for_new_roots(
        16,
        "test_fail_entry_verification_leader",
        SocketAddrSpace::Unspecified,
    );
}

#[test]
#[serial]
#[ignore]
#[allow(unused_attributes)]
fn test_fake_shreds_broadcast_leader() {
    let node_stakes = vec![300, 100];
    let (cluster, _) = test_faulty_node(BroadcastStageType::BroadcastFakeShreds, node_stakes);
    cluster.check_for_new_roots(
        16,
        "test_fake_shreds_broadcast_leader",
        SocketAddrSpace::Unspecified,
    );
}

#[test]
fn test_wait_for_max_stake() {
    solana_logger::setup_with_default(RUST_LOG_FILTER);
    let validator_config = ValidatorConfig::default_for_test();
    let mut config = ClusterConfig {
        cluster_lamports: 10_000,
        node_stakes: vec![100; 4],
        validator_configs: make_identical_validator_configs(&validator_config, 4),
        ..ClusterConfig::default()
    };
    let cluster = LocalCluster::new(&mut config, SocketAddrSpace::Unspecified);
    let client = RpcClient::new_socket(cluster.entry_point_info.rpc);

    assert!(client
        .wait_for_max_stake(CommitmentConfig::default(), 33.0f32)
        .is_ok());
    assert!(client.get_slot().unwrap() > 10);
}

#[test]
// Test that when a leader is leader for banks B_i..B_{i+n}, and B_i is not
// votable, then B_{i+1} still chains to B_i
fn test_no_voting() {
    solana_logger::setup_with_default(RUST_LOG_FILTER);
    let validator_config = ValidatorConfig {
        voting_disabled: true,
        ..ValidatorConfig::default_for_test()
    };
    let mut config = ClusterConfig {
        cluster_lamports: 10_000,
        node_stakes: vec![100],
        validator_configs: vec![validator_config],
        ..ClusterConfig::default()
    };
    let mut cluster = LocalCluster::new(&mut config, SocketAddrSpace::Unspecified);
    let client = cluster
        .get_validator_client(&cluster.entry_point_info.id)
        .unwrap();
    loop {
        let last_slot = client
            .get_slot_with_commitment(CommitmentConfig::processed())
            .expect("Couldn't get slot");
        if last_slot > 4 * VOTE_THRESHOLD_DEPTH as u64 {
            break;
        }
        sleep(Duration::from_secs(1));
    }

    cluster.close_preserve_ledgers();
    let leader_pubkey = cluster.entry_point_info.id;
    let ledger_path = cluster.validators[&leader_pubkey].info.ledger_path.clone();
    let ledger = Blockstore::open(&ledger_path).unwrap();
    for i in 0..2 * VOTE_THRESHOLD_DEPTH {
        let meta = ledger.meta(i as u64).unwrap().unwrap();
        let parent = meta.parent_slot;
        let expected_parent = i.saturating_sub(1);
        assert_eq!(parent, Some(expected_parent as u64));
    }
}

#[test]
#[serial]
fn test_optimistic_confirmation_violation_detection() {
    solana_logger::setup_with_default(RUST_LOG_FILTER);
    // First set up the cluster with 2 nodes
    let slots_per_epoch = 2048;
    let node_stakes = vec![51, 50];
    let validator_keys: Vec<_> = vec![
        "4qhhXNTbKD1a5vxDDLZcHKj7ELNeiivtUBxn3wUK1F5VRsQVP89VUhfXqSfgiFB14GfuBgtrQ96n9NvWQADVkcCg",
        "3kHBzVwie5vTEaY6nFCPeFT8qDpoXzn7dCEioGRNBTnUDpvwnG85w8Wq63gVWpVTP8k2a8cgcWRjSXyUkEygpXWS",
    ]
    .iter()
    .map(|s| (Arc::new(Keypair::from_base58_string(s)), true))
    .take(node_stakes.len())
    .collect();
    let mut config = ClusterConfig {
        cluster_lamports: 100_000,
        node_stakes: node_stakes.clone(),
        validator_configs: make_identical_validator_configs(
            &ValidatorConfig::default_for_test(),
            node_stakes.len(),
        ),
        validator_keys: Some(validator_keys),
        slots_per_epoch,
        stakers_slot_offset: slots_per_epoch,
        skip_warmup_slots: true,
        ..ClusterConfig::default()
    };
    let mut cluster = LocalCluster::new(&mut config, SocketAddrSpace::Unspecified);
    let entry_point_id = cluster.entry_point_info.id;
    // Let the nodes run for a while. Wait for validators to vote on slot `S`
    // so that the vote on `S-1` is definitely in gossip and optimistic confirmation is
    // detected on slot `S-1` for sure, then stop the heavier of the two
    // validators
    let client = cluster.get_validator_client(&entry_point_id).unwrap();
    let mut prev_voted_slot = 0;
    loop {
        let last_voted_slot = client
            .get_slot_with_commitment(CommitmentConfig::processed())
            .unwrap();
        if last_voted_slot > 50 {
            if prev_voted_slot == 0 {
                prev_voted_slot = last_voted_slot;
            } else {
                break;
            }
        }
        sleep(Duration::from_millis(100));
    }

    let exited_validator_info = cluster.exit_node(&entry_point_id);

    // Mark fork as dead on the heavier validator, this should make the fork effectively
    // dead, even though it was optimistically confirmed. The smaller validator should
    // create and jump over to a new fork
    // Also, remove saved tower to intentionally make the restarted validator to violate the
    // optimistic confirmation
    {
        let blockstore = open_blockstore(&exited_validator_info.info.ledger_path);
        info!(
            "Setting slot: {} on main fork as dead, should cause fork",
            prev_voted_slot
        );
        // Necessary otherwise tower will inform this validator that it's latest
        // vote is on slot `prev_voted_slot`. This will then prevent this validator
        // from resetting to the parent of `prev_voted_slot` to create an alternative fork because
        // 1) Validator can't vote on earlier ancestor of last vote due to switch threshold (can't vote
        // on ancestors of last vote)
        // 2) Won't reset to this earlier ancestor becasue reset can only happen on same voted fork if
        // it's for the last vote slot or later
        remove_tower(&exited_validator_info.info.ledger_path, &entry_point_id);
        blockstore.set_dead_slot(prev_voted_slot).unwrap();
    }

    {
        // Buffer stderr to detect optimistic slot violation log
        let buf = std::env::var("OPTIMISTIC_CONF_TEST_DUMP_LOG")
            .err()
            .map(|_| BufferRedirect::stderr().unwrap());
        cluster.restart_node(
            &entry_point_id,
            exited_validator_info,
            SocketAddrSpace::Unspecified,
        );

        // Wait for a root > prev_voted_slot to be set. Because the root is on a
        // different fork than `prev_voted_slot`, then optimistic confirmation is
        // violated
        let client = cluster.get_validator_client(&entry_point_id).unwrap();
        loop {
            let last_root = client
                .get_slot_with_commitment(CommitmentConfig::finalized())
                .unwrap();
            if last_root > prev_voted_slot {
                break;
            }
            sleep(Duration::from_millis(100));
        }

        // Check to see that validator detected optimistic confirmation for
        // `prev_voted_slot` failed
        let expected_log =
            OptimisticConfirmationVerifier::format_optimistic_confirmed_slot_violation_log(
                prev_voted_slot,
            );
        // Violation detection thread can be behind so poll logs up to 10 seconds
        if let Some(mut buf) = buf {
            let start = Instant::now();
            let mut success = false;
            let mut output = String::new();
            while start.elapsed().as_secs() < 10 {
                buf.read_to_string(&mut output).unwrap();
                if output.contains(&expected_log) {
                    success = true;
                    break;
                }
                sleep(Duration::from_millis(10));
            }
            print!("{}", output);
            assert!(success);
        } else {
            panic!("dumped log and disabled testing");
        }
    }

    // Make sure validator still makes progress
    cluster_tests::check_for_new_roots(
        16,
        &[cluster.get_contact_info(&entry_point_id).unwrap().clone()],
        "test_optimistic_confirmation_violation",
    );
}

#[test]
#[serial]
fn test_validator_saves_tower() {
    solana_logger::setup_with_default(RUST_LOG_FILTER);

    let validator_config = ValidatorConfig {
        require_tower: true,
        ..ValidatorConfig::default_for_test()
    };
    let validator_identity_keypair = Arc::new(Keypair::new());
    let validator_id = validator_identity_keypair.pubkey();
    let mut config = ClusterConfig {
        cluster_lamports: 10_000,
        node_stakes: vec![100],
        validator_configs: vec![validator_config],
        validator_keys: Some(vec![(validator_identity_keypair.clone(), true)]),
        ..ClusterConfig::default()
    };
    let mut cluster = LocalCluster::new(&mut config, SocketAddrSpace::Unspecified);

    let validator_client = cluster.get_validator_client(&validator_id).unwrap();

    let ledger_path = cluster
        .validators
        .get(&validator_id)
        .unwrap()
        .info
        .ledger_path
        .clone();

    let file_tower_storage = FileTowerStorage::new(ledger_path.clone());

    // Wait for some votes to be generated
    loop {
        if let Ok(slot) = validator_client.get_slot_with_commitment(CommitmentConfig::processed()) {
            trace!("current slot: {}", slot);
            if slot > 2 {
                break;
            }
        }
        sleep(Duration::from_millis(10));
    }

    // Stop validator and check saved tower
    let validator_info = cluster.exit_node(&validator_id);
    let tower1 = Tower::restore(&file_tower_storage, &validator_id).unwrap();
    trace!("tower1: {:?}", tower1);
    assert_eq!(tower1.root(), 0);
    assert!(tower1.last_voted_slot().is_some());

    // Restart the validator and wait for a new root
    cluster.restart_node(&validator_id, validator_info, SocketAddrSpace::Unspecified);
    let validator_client = cluster.get_validator_client(&validator_id).unwrap();

    // Wait for the first new root
    let last_replayed_root = loop {
        #[allow(deprecated)]
        // This test depends on knowing the immediate root, without any delay from the commitment
        // service, so the deprecated CommitmentConfig::root() is retained
        if let Ok(root) = validator_client.get_slot_with_commitment(CommitmentConfig::root()) {
            trace!("current root: {}", root);
            if root > 0 {
                break root;
            }
        }
        sleep(Duration::from_millis(50));
    };

    // Stop validator, and check saved tower
    let validator_info = cluster.exit_node(&validator_id);
    let tower2 = Tower::restore(&file_tower_storage, &validator_id).unwrap();
    trace!("tower2: {:?}", tower2);
    assert_eq!(tower2.root(), last_replayed_root);

    // Rollback saved tower to `tower1` to simulate a validator starting from a newer snapshot
    // without having to wait for that snapshot to be generated in this test
    tower1
        .save(&file_tower_storage, &validator_identity_keypair)
        .unwrap();

    cluster.restart_node(&validator_id, validator_info, SocketAddrSpace::Unspecified);
    let validator_client = cluster.get_validator_client(&validator_id).unwrap();

    // Wait for a new root, demonstrating the validator was able to make progress from the older `tower1`
    let new_root = loop {
        #[allow(deprecated)]
        // This test depends on knowing the immediate root, without any delay from the commitment
        // service, so the deprecated CommitmentConfig::root() is retained
        if let Ok(root) = validator_client.get_slot_with_commitment(CommitmentConfig::root()) {
            trace!(
                "current root: {}, last_replayed_root: {}",
                root,
                last_replayed_root
            );
            if root > last_replayed_root {
                break root;
            }
        }
        sleep(Duration::from_millis(50));
    };

    // Check the new root is reflected in the saved tower state
    let mut validator_info = cluster.exit_node(&validator_id);
    let tower3 = Tower::restore(&file_tower_storage, &validator_id).unwrap();
    trace!("tower3: {:?}", tower3);
    let tower3_root = tower3.root();
    assert!(tower3_root >= new_root);

    // Remove the tower file entirely and allow the validator to start without a tower.  It will
    // rebuild tower from its vote account contents
    remove_tower(&ledger_path, &validator_id);
    validator_info.config.require_tower = false;

    cluster.restart_node(&validator_id, validator_info, SocketAddrSpace::Unspecified);
    let validator_client = cluster.get_validator_client(&validator_id).unwrap();

    // Wait for another new root
    let new_root = loop {
        #[allow(deprecated)]
        // This test depends on knowing the immediate root, without any delay from the commitment
        // service, so the deprecated CommitmentConfig::root() is retained
        if let Ok(root) = validator_client.get_slot_with_commitment(CommitmentConfig::root()) {
            trace!("current root: {}, last tower root: {}", root, tower3_root);
            if root > tower3_root {
                break root;
            }
        }
        sleep(Duration::from_millis(50));
    };

    cluster.close_preserve_ledgers();

    let tower4 = Tower::restore(&file_tower_storage, &validator_id).unwrap();
    trace!("tower4: {:?}", tower4);
    assert!(tower4.root() >= new_root);
}

fn save_tower(tower_path: &Path, tower: &Tower, node_keypair: &Keypair) {
    let file_tower_storage = FileTowerStorage::new(tower_path.to_path_buf());
    let saved_tower = SavedTower::new(tower, node_keypair).unwrap();
    file_tower_storage
        .store(&SavedTowerVersions::from(saved_tower))
        .unwrap();
}

fn root_in_tower(tower_path: &Path, node_pubkey: &Pubkey) -> Option<Slot> {
    restore_tower(tower_path, node_pubkey).map(|tower| tower.root())
}

// This test verifies that even if votes from a validator end up taking too long to land, and thus
// some of the referenced slots are slots are no longer present in the slot hashes sysvar,
// consensus can still be attained.
//
// Validator A (60%)
// Validator B (40%)
//                                  / --- 10 --- [..] --- 16 (B is voting, due to network issues is initally not able to see the other fork at all)
//                                 /
// 1 - 2 - 3 - 4 - 5 - 6 - 7 - 8 - 9 (A votes 1 - 9 votes are landing normally. B does the same however votes are not landing)
//                                 \
//                                  \--[..]-- 73  (majority fork)
// A is voting on the majority fork and B wants to switch to this fork however in this majority fork
// the earlier votes for B (1 - 9) never landed so when B eventually goes to vote on 73, slots in
// its local vote state are no longer present in slot hashes.
//
// 1. Wait for B's tower to see local vote state was updated to new fork
// 2. Wait X blocks, check B's vote state on chain has been properly updated
//
// NOTE: it is not reliable for B to organically have 1 to reach 2^16 lockout, so we simulate the 6
// consecutive votes on the minor fork by manually incrementing the confirmation levels for the
// common ancestor votes in tower.
// To allow this test to run in a reasonable time we change the
// slot_hash expiry to 64 slots.

#[test]
fn test_slot_hash_expiry() {
    solana_logger::setup_with_default(RUST_LOG_FILTER);
    solana_sdk::slot_hashes::set_entries_for_tests_only(64);

    let slots_per_epoch = 2048;
    let node_stakes = vec![60, 40];
    let validator_keys = vec![
        "28bN3xyvrP4E8LwEgtLjhnkb7cY4amQb6DrYAbAYjgRV4GAGgkVM2K7wnxnAS7WDneuavza7x21MiafLu1HkwQt4",
        "2saHBBoTkLMmttmPQP8KfBkcCw45S5cwtV3wTdGCscRC8uxdgvHxpHiWXKx4LvJjNJtnNcbSv5NdheokFFqnNDt8",
    ]
    .iter()
    .map(|s| (Arc::new(Keypair::from_base58_string(s)), true))
    .collect::<Vec<_>>();
    let node_vote_keys = vec![
        "3NDQ3ud86RTVg8hTy2dDWnS4P8NfjhZ2gDgQAJbr3heaKaUVS1FW3sTLKA1GmDrY9aySzsa4QxpDkbLv47yHxzr3",
        "46ZHpHE6PEvXYPu3hf9iQqjBk2ZNDaJ9ejqKWHEjxaQjpAGasKaWKbKHbP3646oZhfgDRzx95DH9PCBKKsoCVngk",
    ]
    .iter()
    .map(|s| Arc::new(Keypair::from_base58_string(s)))
    .collect::<Vec<_>>();
    let vs = validator_keys
        .iter()
        .map(|(kp, _)| kp.pubkey())
        .collect::<Vec<_>>();
    let (a_pubkey, b_pubkey) = (vs[0], vs[1]);

    // We want B to not vote (we are trying to simulate its votes not landing until it gets to the
    // minority fork)
    let mut validator_configs =
        make_identical_validator_configs(&ValidatorConfig::default_for_test(), node_stakes.len());
    validator_configs[1].voting_disabled = true;

    let mut config = ClusterConfig {
        cluster_lamports: 100_000,
        node_stakes,
        validator_configs,
        validator_keys: Some(validator_keys),
        node_vote_keys: Some(node_vote_keys),
        slots_per_epoch,
        stakers_slot_offset: slots_per_epoch,
        skip_warmup_slots: true,
        ..ClusterConfig::default()
    };
    let mut cluster = LocalCluster::new(&mut config, SocketAddrSpace::Unspecified);

    let mut common_ancestor_slot = 8;

    let a_ledger_path = cluster.ledger_path(&a_pubkey);
    let b_ledger_path = cluster.ledger_path(&b_pubkey);

    // Immediately kill B (we just needed it for the initial stake distribution)
    info!("Killing B");
    let mut b_info = cluster.exit_node(&b_pubkey);

    // Let A run for a while until we get to the common ancestor
    info!("Letting A run until common_ancestor_slot");
    loop {
        if let Some((last_vote, _)) = last_vote_in_tower(&a_ledger_path, &a_pubkey) {
            if last_vote >= common_ancestor_slot {
                break;
            }
        }
        sleep(Duration::from_millis(100));
    }

    // Keep A running, but setup B so that it thinks it has voted up until common ancestor (but
    // doesn't know anything past that)
    {
        info!("Copying A's ledger to B");
        std::fs::remove_dir_all(&b_info.info.ledger_path).unwrap();
        let mut opt = fs_extra::dir::CopyOptions::new();
        opt.copy_inside = true;
        fs_extra::dir::copy(&a_ledger_path, &b_ledger_path, &opt).unwrap();

        // remove A's tower in B's new copied ledger
        info!("Removing A's tower in B's ledger dir");
        remove_tower(&b_ledger_path, &a_pubkey);

        // load A's tower and save it as B's tower
        info!("Loading A's tower");
        if let Some(mut a_tower) = restore_tower(&a_ledger_path, &a_pubkey) {
            a_tower.node_pubkey = b_pubkey;
            // Update common_ancestor_slot because A is still running
            if let Some(s) = a_tower.last_voted_slot() {
                common_ancestor_slot = s;
                info!("New common_ancestor_slot {}", common_ancestor_slot);
            } else {
                panic!("A's tower has no votes");
            }
            info!("Increase lockout by 6 confirmation levels and save as B's tower");
            a_tower.increase_lockout(6);
            save_tower(&b_ledger_path, &a_tower, &b_info.info.keypair);
            info!("B's new tower: {:?}", a_tower.tower_slots());
        } else {
            panic!("A's tower is missing");
        }

        // Get rid of any slots past common_ancestor_slot
        info!("Removing extra slots from B's blockstore");
        let blockstore = open_blockstore(&b_ledger_path);
        purge_slots(&blockstore, common_ancestor_slot + 1, 100);
    }

    info!(
        "Run A on majority fork until it reaches slot hash expiry {}",
        solana_sdk::slot_hashes::get_entries()
    );
    let mut last_vote_on_a;
    // Keep A running for a while longer so the majority fork has some decent size
    loop {
        last_vote_on_a = wait_for_last_vote_in_tower_to_land_in_ledger(&a_ledger_path, &a_pubkey);
        if last_vote_on_a
            >= common_ancestor_slot + 2 * (solana_sdk::slot_hashes::get_entries() as u64)
        {
            let blockstore = open_blockstore(&a_ledger_path);
            info!(
                "A majority fork: {:?}",
                AncestorIterator::new(last_vote_on_a, &blockstore).collect::<Vec<Slot>>()
            );
            break;
        }
        sleep(Duration::from_millis(100));
    }

    // Kill A and restart B with voting. B should now fork off
    info!("Killing A");
    let a_info = cluster.exit_node(&a_pubkey);

    info!("Restarting B");
    b_info.config.voting_disabled = false;
    cluster.restart_node(&b_pubkey, b_info, SocketAddrSpace::Unspecified);

    // B will fork off and accumulate enough lockout
    info!("Allowing B to fork");
    loop {
        let blockstore = open_blockstore(&b_ledger_path);
        let last_vote = wait_for_last_vote_in_tower_to_land_in_ledger(&b_ledger_path, &b_pubkey);
        let mut ancestors = AncestorIterator::new(last_vote, &blockstore);
        if let Some(index) = ancestors.position(|x| x == common_ancestor_slot) {
            if index > 7 {
                info!(
                    "B has forked for enough lockout: {:?}",
                    AncestorIterator::new(last_vote, &blockstore).collect::<Vec<Slot>>()
                );
                break;
            }
        }
        sleep(Duration::from_millis(1000));
    }

    info!("Kill B");
    b_info = cluster.exit_node(&b_pubkey);

    info!("Resolve the partition");
    {
        // Here we let B know about the missing blocks that A had produced on its partition
        let a_blockstore = open_blockstore(&a_ledger_path);
        let b_blockstore = open_blockstore(&b_ledger_path);
        copy_blocks(last_vote_on_a, &a_blockstore, &b_blockstore);
    }

    // Now restart A and B and see if B is able to eventually switch onto the majority fork
    info!("Restarting A & B");
    cluster.restart_node(&a_pubkey, a_info, SocketAddrSpace::Unspecified);
    cluster.restart_node(&b_pubkey, b_info, SocketAddrSpace::Unspecified);

    info!("Waiting for B to switch to majority fork and make a root");
    cluster_tests::check_for_new_roots(
        16,
        &[cluster.get_contact_info(&a_pubkey).unwrap().clone()],
        "test_slot_hashes_expiry",
    );
}

enum ClusterMode {
    MasterOnly,
    MasterSlave,
}

fn do_test_future_tower(cluster_mode: ClusterMode) {
    solana_logger::setup_with_default(RUST_LOG_FILTER);

    // First set up the cluster with 4 nodes
    let slots_per_epoch = 2048;
    let node_stakes = match cluster_mode {
        ClusterMode::MasterOnly => vec![100],
        ClusterMode::MasterSlave => vec![100, 1],
    };

    let validator_keys = vec![
        "28bN3xyvrP4E8LwEgtLjhnkb7cY4amQb6DrYAbAYjgRV4GAGgkVM2K7wnxnAS7WDneuavza7x21MiafLu1HkwQt4",
        "2saHBBoTkLMmttmPQP8KfBkcCw45S5cwtV3wTdGCscRC8uxdgvHxpHiWXKx4LvJjNJtnNcbSv5NdheokFFqnNDt8",
    ]
    .iter()
    .map(|s| (Arc::new(Keypair::from_base58_string(s)), true))
    .take(node_stakes.len())
    .collect::<Vec<_>>();
    let validators = validator_keys
        .iter()
        .map(|(kp, _)| kp.pubkey())
        .collect::<Vec<_>>();
    let validator_a_pubkey = match cluster_mode {
        ClusterMode::MasterOnly => validators[0],
        ClusterMode::MasterSlave => validators[1],
    };

    let mut config = ClusterConfig {
        cluster_lamports: 100_000,
        node_stakes: node_stakes.clone(),
        validator_configs: make_identical_validator_configs(
            &ValidatorConfig::default_for_test(),
            node_stakes.len(),
        ),
        validator_keys: Some(validator_keys),
        slots_per_epoch,
        stakers_slot_offset: slots_per_epoch,
        skip_warmup_slots: true,
        ..ClusterConfig::default()
    };
    let mut cluster = LocalCluster::new(&mut config, SocketAddrSpace::Unspecified);

    let val_a_ledger_path = cluster.ledger_path(&validator_a_pubkey);

    loop {
        sleep(Duration::from_millis(100));

        if let Some(root) = root_in_tower(&val_a_ledger_path, &validator_a_pubkey) {
            if root >= 15 {
                break;
            }
        }
    }
    let purged_slot_before_restart = 10;
    let validator_a_info = cluster.exit_node(&validator_a_pubkey);
    {
        // create a warped future tower without mangling the tower itself
        info!(
            "Revert blockstore before slot {} and effectively create a future tower",
            purged_slot_before_restart,
        );
        let blockstore = open_blockstore(&val_a_ledger_path);
        purge_slots(&blockstore, purged_slot_before_restart, 100);
    }

    cluster.restart_node(
        &validator_a_pubkey,
        validator_a_info,
        SocketAddrSpace::Unspecified,
    );

    let mut newly_rooted = false;
    let some_root_after_restart = purged_slot_before_restart + 25; // 25 is arbitrary; just wait a bit
    for _ in 0..600 {
        sleep(Duration::from_millis(100));

        if let Some(root) = root_in_tower(&val_a_ledger_path, &validator_a_pubkey) {
            if root >= some_root_after_restart {
                newly_rooted = true;
                break;
            }
        }
    }
    let _validator_a_info = cluster.exit_node(&validator_a_pubkey);
    if newly_rooted {
        // there should be no forks; i.e. monotonically increasing ancestor chain
        let (last_vote, _) = last_vote_in_tower(&val_a_ledger_path, &validator_a_pubkey).unwrap();
        let blockstore = open_blockstore(&val_a_ledger_path);
        let actual_block_ancestors = AncestorIterator::new_inclusive(last_vote, &blockstore)
            .take_while(|a| *a >= some_root_after_restart)
            .collect::<Vec<_>>();
        let expected_countinuous_no_fork_votes = (some_root_after_restart..=last_vote)
            .rev()
            .collect::<Vec<_>>();
        assert_eq!(actual_block_ancestors, expected_countinuous_no_fork_votes);
        assert!(actual_block_ancestors.len() > MAX_LOCKOUT_HISTORY);
        info!("validator managed to handle future tower!");
    } else {
        panic!("no root detected");
    }
}

#[test]
#[serial]
fn test_future_tower_master_only() {
    do_test_future_tower(ClusterMode::MasterOnly);
}

#[test]
#[serial]
fn test_future_tower_master_slave() {
    do_test_future_tower(ClusterMode::MasterSlave);
}

#[test]
fn test_hard_fork_invalidates_tower() {
    solana_logger::setup_with_default(RUST_LOG_FILTER);

    // First set up the cluster with 2 nodes
    let slots_per_epoch = 2048;
    let node_stakes = vec![60, 40];

    let validator_keys = vec![
        "28bN3xyvrP4E8LwEgtLjhnkb7cY4amQb6DrYAbAYjgRV4GAGgkVM2K7wnxnAS7WDneuavza7x21MiafLu1HkwQt4",
        "2saHBBoTkLMmttmPQP8KfBkcCw45S5cwtV3wTdGCscRC8uxdgvHxpHiWXKx4LvJjNJtnNcbSv5NdheokFFqnNDt8",
    ]
    .iter()
    .map(|s| (Arc::new(Keypair::from_base58_string(s)), true))
    .take(node_stakes.len())
    .collect::<Vec<_>>();
    let validators = validator_keys
        .iter()
        .map(|(kp, _)| kp.pubkey())
        .collect::<Vec<_>>();

    let validator_a_pubkey = validators[0];
    let validator_b_pubkey = validators[1];

    let mut config = ClusterConfig {
        cluster_lamports: 100_000,
        node_stakes: node_stakes.clone(),
        validator_configs: make_identical_validator_configs(
            &ValidatorConfig::default_for_test(),
            node_stakes.len(),
        ),
        validator_keys: Some(validator_keys),
        slots_per_epoch,
        stakers_slot_offset: slots_per_epoch,
        skip_warmup_slots: true,
        ..ClusterConfig::default()
    };
    let cluster = std::sync::Arc::new(std::sync::Mutex::new(LocalCluster::new(
        &mut config,
        SocketAddrSpace::Unspecified,
    )));

    let val_a_ledger_path = cluster.lock().unwrap().ledger_path(&validator_a_pubkey);

    let min_root = 15;
    loop {
        sleep(Duration::from_millis(100));

        if let Some(root) = root_in_tower(&val_a_ledger_path, &validator_a_pubkey) {
            if root >= min_root {
                break;
            }
        }
    }

    let mut validator_a_info = cluster.lock().unwrap().exit_node(&validator_a_pubkey);
    let mut validator_b_info = cluster.lock().unwrap().exit_node(&validator_b_pubkey);

    // setup hard fork at slot < a previously rooted slot!
    let hard_fork_slot = min_root - 5;
    let hard_fork_slots = Some(vec![hard_fork_slot]);
    let mut hard_forks = solana_sdk::hard_forks::HardForks::default();
    hard_forks.register(hard_fork_slot);

    let expected_shred_version = solana_sdk::shred_version::compute_shred_version(
        &cluster.lock().unwrap().genesis_config.hash(),
        Some(&hard_forks),
    );

    validator_a_info.config.new_hard_forks = hard_fork_slots.clone();
    validator_a_info.config.wait_for_supermajority = Some(hard_fork_slot);
    validator_a_info.config.expected_shred_version = Some(expected_shred_version);

    validator_b_info.config.new_hard_forks = hard_fork_slots;
    validator_b_info.config.wait_for_supermajority = Some(hard_fork_slot);
    validator_b_info.config.expected_shred_version = Some(expected_shred_version);

    // Clear ledger of all slots post hard fork
    {
        let blockstore_a = open_blockstore(&validator_a_info.info.ledger_path);
        let blockstore_b = open_blockstore(&validator_b_info.info.ledger_path);
        purge_slots(&blockstore_a, hard_fork_slot + 1, 100);
        purge_slots(&blockstore_b, hard_fork_slot + 1, 100);
    }

    // restart validator A first
    let cluster_for_a = cluster.clone();
    // Spawn a thread because wait_for_supermajority blocks in Validator::new()!
    let thread = std::thread::spawn(move || {
        let restart_context = cluster_for_a
            .lock()
            .unwrap()
            .create_restart_context(&validator_a_pubkey, &mut validator_a_info);
        let restarted_validator_info = LocalCluster::restart_node_with_context(
            validator_a_info,
            restart_context,
            SocketAddrSpace::Unspecified,
        );
        cluster_for_a
            .lock()
            .unwrap()
            .add_node(&validator_a_pubkey, restarted_validator_info);
    });

    // test validator A actually to wait for supermajority
    let mut last_vote = None;
    for _ in 0..10 {
        sleep(Duration::from_millis(1000));

        let (new_last_vote, _) =
            last_vote_in_tower(&val_a_ledger_path, &validator_a_pubkey).unwrap();
        if let Some(last_vote) = last_vote {
            assert_eq!(last_vote, new_last_vote);
        } else {
            last_vote = Some(new_last_vote);
        }
    }

    // restart validator B normally
    cluster.lock().unwrap().restart_node(
        &validator_b_pubkey,
        validator_b_info,
        SocketAddrSpace::Unspecified,
    );

    // validator A should now start so join its thread here
    thread.join().unwrap();

    // new slots should be rooted after hard-fork cluster relaunch
    cluster
        .lock()
        .unwrap()
        .check_for_new_roots(16, "hard fork", SocketAddrSpace::Unspecified);
}

#[test]
#[serial]
fn test_run_test_load_program_accounts_root() {
    run_test_load_program_accounts(CommitmentConfig::finalized());
}

#[test]
#[serial]
fn test_restart_tower_rollback() {
    // Test node crashing and failing to save its tower before restart
    // Cluster continues to make progress, this node is able to rejoin with
    // outdated tower post restart.
    solana_logger::setup_with_default(RUST_LOG_FILTER);

    // First set up the cluster with 2 nodes
    let slots_per_epoch = 2048;
    let node_stakes = vec![10000, 1];

    let validator_strings = vec![
        "28bN3xyvrP4E8LwEgtLjhnkb7cY4amQb6DrYAbAYjgRV4GAGgkVM2K7wnxnAS7WDneuavza7x21MiafLu1HkwQt4",
        "2saHBBoTkLMmttmPQP8KfBkcCw45S5cwtV3wTdGCscRC8uxdgvHxpHiWXKx4LvJjNJtnNcbSv5NdheokFFqnNDt8",
    ];

    let validator_keys = validator_strings
        .iter()
        .map(|s| (Arc::new(Keypair::from_base58_string(s)), true))
        .take(node_stakes.len())
        .collect::<Vec<_>>();

    let b_pubkey = validator_keys[1].0.pubkey();

    let mut config = ClusterConfig {
        cluster_lamports: 100_000,
        node_stakes: node_stakes.clone(),
        validator_configs: make_identical_validator_configs(
            &ValidatorConfig::default_for_test(),
            node_stakes.len(),
        ),
        validator_keys: Some(validator_keys),
        slots_per_epoch,
        stakers_slot_offset: slots_per_epoch,
        skip_warmup_slots: true,
        ..ClusterConfig::default()
    };
    let mut cluster = LocalCluster::new(&mut config, SocketAddrSpace::Unspecified);

    let val_b_ledger_path = cluster.ledger_path(&b_pubkey);

    let mut earlier_tower: Tower;
    loop {
        sleep(Duration::from_millis(1000));

        // Grab the current saved tower
        earlier_tower = restore_tower(&val_b_ledger_path, &b_pubkey).unwrap();
        if earlier_tower.last_voted_slot().unwrap_or(0) > 1 {
            break;
        }
    }

    let mut exited_validator_info: ClusterValidatorInfo;
    let last_voted_slot: Slot;
    loop {
        sleep(Duration::from_millis(1000));

        // Wait for second, lesser staked validator to make a root past the earlier_tower's
        // latest vote slot, then exit that validator
        let tower = restore_tower(&val_b_ledger_path, &b_pubkey).unwrap();
        if tower.root()
            > earlier_tower
                .last_voted_slot()
                .expect("Earlier tower must have at least one vote")
        {
            exited_validator_info = cluster.exit_node(&b_pubkey);
            last_voted_slot = tower.last_voted_slot().unwrap();
            break;
        }
    }

    // Now rewrite the tower with the *earlier_tower*. We disable voting until we reach
    // a slot we did not previously vote for in order to avoid duplicate vote slashing
    // issues.
    save_tower(
        &val_b_ledger_path,
        &earlier_tower,
        &exited_validator_info.info.keypair,
    );
    exited_validator_info.config.wait_to_vote_slot = Some(last_voted_slot + 10);

    cluster.restart_node(
        &b_pubkey,
        exited_validator_info,
        SocketAddrSpace::Unspecified,
    );

    // Check this node is making new roots
    cluster.check_for_new_roots(
        20,
        "test_restart_tower_rollback",
        SocketAddrSpace::Unspecified,
    );
}

#[test]
#[serial]
fn test_run_test_load_program_accounts_partition_root() {
    run_test_load_program_accounts_partition(CommitmentConfig::finalized());
}

fn run_test_load_program_accounts_partition(scan_commitment: CommitmentConfig) {
    let num_slots_per_validator = 8;
    let partitions: [Vec<usize>; 2] = [vec![1], vec![1]];
    let (leader_schedule, validator_keys) = create_custom_leader_schedule_with_random_keys(&[
        num_slots_per_validator,
        num_slots_per_validator,
    ]);

    let (update_client_sender, update_client_receiver) = unbounded();
    let (scan_client_sender, scan_client_receiver) = unbounded();
    let exit = Arc::new(AtomicBool::new(false));

    let (t_update, t_scan, additional_accounts) = setup_transfer_scan_threads(
        1000,
        exit.clone(),
        scan_commitment,
        update_client_receiver,
        scan_client_receiver,
    );

    let on_partition_start = |cluster: &mut LocalCluster, _: &mut ()| {
        let update_client = cluster
            .get_validator_client(&cluster.entry_point_info.id)
            .unwrap();
        update_client_sender.send(update_client).unwrap();
        let scan_client = cluster
            .get_validator_client(&cluster.entry_point_info.id)
            .unwrap();
        scan_client_sender.send(scan_client).unwrap();
    };

    let on_partition_before_resolved = |_: &mut LocalCluster, _: &mut ()| {};

    let on_partition_resolved = |cluster: &mut LocalCluster, _: &mut ()| {
        cluster.check_for_new_roots(
            20,
            "run_test_load_program_accounts_partition",
            SocketAddrSpace::Unspecified,
        );
        exit.store(true, Ordering::Relaxed);
        t_update.join().unwrap();
        t_scan.join().unwrap();
    };

    run_cluster_partition(
        &partitions,
        Some((leader_schedule, validator_keys)),
        (),
        on_partition_start,
        on_partition_before_resolved,
        on_partition_resolved,
        None,
        None,
        additional_accounts,
    );
}

#[test]
#[serial]
fn test_votes_land_in_fork_during_long_partition() {
    let total_stake = 100;
    // Make `lighter_stake` insufficient for switching threshold
    let lighter_stake = (SWITCH_FORK_THRESHOLD as f64 * total_stake as f64) as u64;
    let heavier_stake = lighter_stake + 1;
    let failures_stake = total_stake - lighter_stake - heavier_stake;

    // Give lighter stake 30 consecutive slots before
    // the heavier stake gets a single slot
    let partitions: &[&[(usize, usize)]] = &[
        &[(heavier_stake as usize, 1)],
        &[(lighter_stake as usize, 30)],
    ];

    #[derive(Default)]
    struct PartitionContext {
        heaviest_validator_key: Pubkey,
        lighter_validator_key: Pubkey,
        heavier_fork_slot: Slot,
    }

    let on_partition_start = |_cluster: &mut LocalCluster,
                              validator_keys: &[Pubkey],
                              _dead_validator_infos: Vec<ClusterValidatorInfo>,
                              context: &mut PartitionContext| {
        // validator_keys[0] is the validator that will be killed, i.e. the validator with
        // stake == `failures_stake`
        context.heaviest_validator_key = validator_keys[1];
        context.lighter_validator_key = validator_keys[2];
    };

    let on_before_partition_resolved =
        |cluster: &mut LocalCluster, context: &mut PartitionContext| {
            let lighter_validator_ledger_path = cluster.ledger_path(&context.lighter_validator_key);
            let heavier_validator_ledger_path =
                cluster.ledger_path(&context.heaviest_validator_key);

            // Wait for each node to have created and voted on its own partition
            loop {
                let (heavier_validator_latest_vote_slot, _) = last_vote_in_tower(
                    &heavier_validator_ledger_path,
                    &context.heaviest_validator_key,
                )
                .unwrap();
                info!(
                    "Checking heavier validator's last vote {} is on a separate fork",
                    heavier_validator_latest_vote_slot
                );
                let lighter_validator_blockstore = open_blockstore(&lighter_validator_ledger_path);
                if lighter_validator_blockstore
                    .meta(heavier_validator_latest_vote_slot)
                    .unwrap()
                    .is_none()
                {
                    context.heavier_fork_slot = heavier_validator_latest_vote_slot;
                    return;
                }
                sleep(Duration::from_millis(100));
            }
        };

    let on_partition_resolved = |cluster: &mut LocalCluster, context: &mut PartitionContext| {
        let lighter_validator_ledger_path = cluster.ledger_path(&context.lighter_validator_key);
        let start = Instant::now();
        let max_wait = ms_for_n_slots(MAX_PROCESSING_AGE as u64, DEFAULT_TICKS_PER_SLOT);
        // Wait for the lighter node to switch over and root the `context.heavier_fork_slot`
        loop {
            assert!(
                // Should finish faster than if the cluster were relying on replay vote
                // refreshing to refresh the vote on blockhash expiration for the vote
                // transaction.
                start.elapsed() <= Duration::from_millis(max_wait),
                "Went too long {} ms without a root",
                max_wait,
            );
            let lighter_validator_blockstore = open_blockstore(&lighter_validator_ledger_path);
            if lighter_validator_blockstore.is_root(context.heavier_fork_slot) {
                info!(
                    "Partition resolved, new root made in {}ms",
                    start.elapsed().as_millis()
                );
                return;
            }
            sleep(Duration::from_millis(100));
        }
    };

    run_kill_partition_switch_threshold(
        &[&[(failures_stake as usize, 0)]],
        partitions,
        None,
        None,
        PartitionContext::default(),
        on_partition_start,
        on_before_partition_resolved,
        on_partition_resolved,
    );
}

fn setup_transfer_scan_threads(
    num_starting_accounts: usize,
    exit: Arc<AtomicBool>,
    scan_commitment: CommitmentConfig,
    update_client_receiver: Receiver<ThinClient>,
    scan_client_receiver: Receiver<ThinClient>,
) -> (
    JoinHandle<()>,
    JoinHandle<()>,
    Vec<(Pubkey, AccountSharedData)>,
) {
    let exit_ = exit.clone();
    let starting_keypairs: Arc<Vec<Keypair>> = Arc::new(
        iter::repeat_with(Keypair::new)
            .take(num_starting_accounts)
            .collect(),
    );
    let target_keypairs: Arc<Vec<Keypair>> = Arc::new(
        iter::repeat_with(Keypair::new)
            .take(num_starting_accounts)
            .collect(),
    );
    let starting_accounts: Vec<(Pubkey, AccountSharedData)> = starting_keypairs
        .iter()
        .map(|k| {
            (
                k.pubkey(),
                AccountSharedData::new(1, 0, &system_program::id()),
            )
        })
        .collect();

    let starting_keypairs_ = starting_keypairs.clone();
    let target_keypairs_ = target_keypairs.clone();
    let t_update = Builder::new()
        .name("update".to_string())
        .spawn(move || {
            let client = update_client_receiver.recv().unwrap();
            loop {
                if exit_.load(Ordering::Relaxed) {
                    return;
                }
                let (blockhash, _) = client
                    .get_latest_blockhash_with_commitment(CommitmentConfig::processed())
                    .unwrap();
                for i in 0..starting_keypairs_.len() {
                    client
                        .async_transfer(
                            1,
                            &starting_keypairs_[i],
                            &target_keypairs_[i].pubkey(),
                            blockhash,
                        )
                        .unwrap();
                }
                for i in 0..starting_keypairs_.len() {
                    client
                        .async_transfer(
                            1,
                            &target_keypairs_[i],
                            &starting_keypairs_[i].pubkey(),
                            blockhash,
                        )
                        .unwrap();
                }
            }
        })
        .unwrap();

    // Scan, the total funds should add up to the original
    let mut scan_commitment_config = RpcProgramAccountsConfig::default();
    scan_commitment_config.account_config.commitment = Some(scan_commitment);
    let tracked_pubkeys: HashSet<Pubkey> = starting_keypairs
        .iter()
        .chain(target_keypairs.iter())
        .map(|k| k.pubkey())
        .collect();
    let expected_total_balance = num_starting_accounts as u64;
    let t_scan = Builder::new()
        .name("scan".to_string())
        .spawn(move || {
            let client = scan_client_receiver.recv().unwrap();
            loop {
                if exit.load(Ordering::Relaxed) {
                    return;
                }
                if let Some(total_scan_balance) = client
                    .get_program_accounts_with_config(
                        &system_program::id(),
                        scan_commitment_config.clone(),
                    )
                    .ok()
                    .map(|result| {
                        result
                            .into_iter()
                            .map(|(key, account)| {
                                if tracked_pubkeys.contains(&key) {
                                    account.lamports
                                } else {
                                    0
                                }
                            })
                            .sum::<u64>()
                    })
                {
                    assert_eq!(total_scan_balance, expected_total_balance);
                }
            }
        })
        .unwrap();

    (t_update, t_scan, starting_accounts)
}

fn run_test_load_program_accounts(scan_commitment: CommitmentConfig) {
    solana_logger::setup_with_default(RUST_LOG_FILTER);
    // First set up the cluster with 2 nodes
    let slots_per_epoch = 2048;
    let node_stakes = vec![51, 50];
    let validator_keys: Vec<_> = vec![
        "4qhhXNTbKD1a5vxDDLZcHKj7ELNeiivtUBxn3wUK1F5VRsQVP89VUhfXqSfgiFB14GfuBgtrQ96n9NvWQADVkcCg",
        "3kHBzVwie5vTEaY6nFCPeFT8qDpoXzn7dCEioGRNBTnUDpvwnG85w8Wq63gVWpVTP8k2a8cgcWRjSXyUkEygpXWS",
    ]
    .iter()
    .map(|s| (Arc::new(Keypair::from_base58_string(s)), true))
    .take(node_stakes.len())
    .collect();

    let num_starting_accounts = 1000;
    let exit = Arc::new(AtomicBool::new(false));
    let (update_client_sender, update_client_receiver) = unbounded();
    let (scan_client_sender, scan_client_receiver) = unbounded();

    // Setup the update/scan threads
    let (t_update, t_scan, starting_accounts) = setup_transfer_scan_threads(
        num_starting_accounts,
        exit.clone(),
        scan_commitment,
        update_client_receiver,
        scan_client_receiver,
    );

    let mut config = ClusterConfig {
        cluster_lamports: 100_000,
        node_stakes: node_stakes.clone(),
        validator_configs: make_identical_validator_configs(
            &ValidatorConfig::default_for_test(),
            node_stakes.len(),
        ),
        validator_keys: Some(validator_keys),
        slots_per_epoch,
        stakers_slot_offset: slots_per_epoch,
        skip_warmup_slots: true,
        additional_accounts: starting_accounts,
        ..ClusterConfig::default()
    };
    let cluster = LocalCluster::new(&mut config, SocketAddrSpace::Unspecified);

    // Give the threads a client to use for querying the cluster
    let all_pubkeys = cluster.get_node_pubkeys();
    let other_validator_id = all_pubkeys
        .into_iter()
        .find(|x| *x != cluster.entry_point_info.id)
        .unwrap();
    let client = cluster
        .get_validator_client(&cluster.entry_point_info.id)
        .unwrap();
    update_client_sender.send(client).unwrap();
    let scan_client = cluster.get_validator_client(&other_validator_id).unwrap();
    scan_client_sender.send(scan_client).unwrap();

    // Wait for some roots to pass
    cluster.check_for_new_roots(
        40,
        "run_test_load_program_accounts",
        SocketAddrSpace::Unspecified,
    );

    // Exit and ensure no violations of consistency were found
    exit.store(true, Ordering::Relaxed);
    t_update.join().unwrap();
    t_scan.join().unwrap();
}

fn farf_dir() -> PathBuf {
    std::env::var("FARF_DIR")
        .unwrap_or_else(|_| "farf".to_string())
        .into()
}

fn generate_account_paths(num_account_paths: usize) -> (Vec<TempDir>, Vec<PathBuf>) {
    let account_storage_dirs: Vec<TempDir> = (0..num_account_paths)
        .map(|_| tempfile::tempdir_in(farf_dir()).unwrap())
        .collect();
    let account_storage_paths: Vec<_> = account_storage_dirs
        .iter()
        .map(|a| a.path().to_path_buf())
        .collect();
    (account_storage_dirs, account_storage_paths)
}

struct SnapshotValidatorConfig {
    bank_snapshots_dir: TempDir,
    snapshot_archives_dir: TempDir,
    account_storage_dirs: Vec<TempDir>,
    validator_config: ValidatorConfig,
}

impl SnapshotValidatorConfig {
    pub fn new(
        full_snapshot_archive_interval_slots: Slot,
        incremental_snapshot_archive_interval_slots: Slot,
        accounts_hash_interval_slots: Slot,
        num_account_paths: usize,
    ) -> SnapshotValidatorConfig {
        assert!(accounts_hash_interval_slots > 0);
        assert!(full_snapshot_archive_interval_slots > 0);
        assert!(full_snapshot_archive_interval_slots % accounts_hash_interval_slots == 0);
        if incremental_snapshot_archive_interval_slots != Slot::MAX {
            assert!(incremental_snapshot_archive_interval_slots > 0);
            assert!(
                incremental_snapshot_archive_interval_slots % accounts_hash_interval_slots == 0
            );
            assert!(
                full_snapshot_archive_interval_slots % incremental_snapshot_archive_interval_slots
                    == 0
            );
        }

        // Create the snapshot config
        let bank_snapshots_dir = tempfile::tempdir_in(farf_dir()).unwrap();
        let snapshot_archives_dir = tempfile::tempdir_in(farf_dir()).unwrap();
        let snapshot_config = SnapshotConfig {
            full_snapshot_archive_interval_slots,
            incremental_snapshot_archive_interval_slots,
            snapshot_archives_dir: snapshot_archives_dir.path().to_path_buf(),
            bank_snapshots_dir: bank_snapshots_dir.path().to_path_buf(),
            ..SnapshotConfig::default()
        };

        // Create the account paths
        let (account_storage_dirs, account_storage_paths) =
            generate_account_paths(num_account_paths);

        // Create the validator config
        let validator_config = ValidatorConfig {
            snapshot_config: Some(snapshot_config),
            account_paths: account_storage_paths,
            accounts_hash_interval_slots,
            ..ValidatorConfig::default_for_test()
        };

        SnapshotValidatorConfig {
            bank_snapshots_dir,
            snapshot_archives_dir,
            account_storage_dirs,
            validator_config,
        }
    }
}

fn setup_snapshot_validator_config(
    snapshot_interval_slots: Slot,
    num_account_paths: usize,
) -> SnapshotValidatorConfig {
    SnapshotValidatorConfig::new(
        snapshot_interval_slots,
        Slot::MAX,
        snapshot_interval_slots,
        num_account_paths,
    )
}
