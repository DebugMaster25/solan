use {
    crate::{
        cluster::{Cluster, ClusterValidatorInfo, QuicTpuClient, ValidatorInfo},
        cluster_tests,
        integration_tests::DEFAULT_NODE_STAKE,
        validator_configs::*,
    },
    itertools::izip,
    log::*,
    solana_accounts_db::utils::create_accounts_run_and_snapshot_dirs,
    solana_client::connection_cache::ConnectionCache,
    solana_core::{
        consensus::tower_storage::FileTowerStorage,
        validator::{Validator, ValidatorConfig, ValidatorStartProgress},
    },
    solana_gossip::{
        cluster_info::Node,
        contact_info::{ContactInfo, Protocol},
        gossip_service::discover_cluster,
    },
    solana_ledger::{create_new_tmp_ledger_with_size, shred::Shred},
    solana_rpc_client::rpc_client::RpcClient,
    solana_runtime::{
        genesis_utils::{
            create_genesis_config_with_vote_accounts_and_cluster_type, GenesisConfigInfo,
            ValidatorVoteKeypairs,
        },
        snapshot_config::SnapshotConfig,
    },
    solana_sdk::{
        account::{Account, AccountSharedData},
        clock::{Slot, DEFAULT_DEV_SLOTS_PER_EPOCH, DEFAULT_TICKS_PER_SLOT, MAX_PROCESSING_AGE},
        commitment_config::CommitmentConfig,
        epoch_schedule::EpochSchedule,
        feature_set,
        genesis_config::{ClusterType, GenesisConfig},
        message::Message,
        poh_config::PohConfig,
        pubkey::Pubkey,
        signature::{Keypair, Signature, Signer},
        signers::Signers,
        stake::{
            instruction as stake_instruction,
            state::{Authorized, Lockup},
        },
        system_transaction,
        transaction::Transaction,
        transport::TransportError,
    },
    solana_stake_program::stake_state,
    solana_streamer::{socket::SocketAddrSpace, streamer::StakedNodes},
    solana_tpu_client::tpu_client::{
        TpuClient, TpuClientConfig, DEFAULT_TPU_CONNECTION_POOL_SIZE, DEFAULT_TPU_ENABLE_UDP,
        DEFAULT_TPU_USE_QUIC,
    },
    solana_vote_program::{
        vote_instruction,
        vote_state::{self, VoteInit},
    },
    std::{
        collections::HashMap,
        io::{Error, ErrorKind, Result},
        iter,
        net::{IpAddr, Ipv4Addr, UdpSocket},
        path::{Path, PathBuf},
        sync::{Arc, RwLock},
        time::Instant,
    },
};

const DUMMY_SNAPSHOT_CONFIG_PATH_MARKER: &str = "dummy";

pub struct ClusterConfig {
    /// The validator config that should be applied to every node in the cluster
    pub validator_configs: Vec<ValidatorConfig>,
    /// Number of nodes that are unstaked and not voting (a.k.a listening)
    pub num_listeners: u64,
    /// List of tuples (pubkeys, in_genesis) of each node if specified. If
    /// `in_genesis` == true, the validator's vote and stake accounts
    //  will be inserted into the genesis block instead of warming up through
    // creating the vote accounts. The first validator (bootstrap leader) automatically
    // is assumed to be `in_genesis` == true.
    pub validator_keys: Option<Vec<(Arc<Keypair>, bool)>>,
    /// The stakes of each node
    pub node_stakes: Vec<u64>,
    /// Optional vote keypairs to use for each node
    pub node_vote_keys: Option<Vec<Arc<Keypair>>>,
    /// The total lamports available to the cluster
    pub cluster_lamports: u64,
    pub ticks_per_slot: u64,
    pub slots_per_epoch: u64,
    pub stakers_slot_offset: u64,
    pub skip_warmup_slots: bool,
    pub native_instruction_processors: Vec<(String, Pubkey)>,
    pub cluster_type: ClusterType,
    pub poh_config: PohConfig,
    pub additional_accounts: Vec<(Pubkey, AccountSharedData)>,
    pub tpu_use_quic: bool,
    pub tpu_connection_pool_size: usize,
}

impl ClusterConfig {
    pub fn new_with_equal_stakes(
        num_nodes: usize,
        cluster_lamports: u64,
        lamports_per_node: u64,
    ) -> Self {
        Self {
            node_stakes: vec![lamports_per_node; num_nodes],
            cluster_lamports,
            validator_configs: make_identical_validator_configs(
                &ValidatorConfig::default_for_test(),
                num_nodes,
            ),
            ..Self::default()
        }
    }
}

impl Default for ClusterConfig {
    fn default() -> Self {
        ClusterConfig {
            validator_configs: vec![],
            num_listeners: 0,
            validator_keys: None,
            node_stakes: vec![],
            node_vote_keys: None,
            cluster_lamports: 0,
            ticks_per_slot: DEFAULT_TICKS_PER_SLOT,
            slots_per_epoch: DEFAULT_DEV_SLOTS_PER_EPOCH,
            stakers_slot_offset: DEFAULT_DEV_SLOTS_PER_EPOCH,
            native_instruction_processors: vec![],
            cluster_type: ClusterType::Development,
            poh_config: PohConfig::default(),
            skip_warmup_slots: false,
            additional_accounts: vec![],
            tpu_use_quic: DEFAULT_TPU_USE_QUIC,
            tpu_connection_pool_size: DEFAULT_TPU_CONNECTION_POOL_SIZE,
        }
    }
}

pub struct LocalCluster {
    /// Keypair with funding to participate in the network
    pub funding_keypair: Keypair,
    /// Entry point from which the rest of the network can be discovered
    pub entry_point_info: ContactInfo,
    pub validators: HashMap<Pubkey, ClusterValidatorInfo>,
    pub genesis_config: GenesisConfig,
    pub connection_cache: Arc<ConnectionCache>,
}

impl LocalCluster {
    pub fn new_with_equal_stakes(
        num_nodes: usize,
        cluster_lamports: u64,
        lamports_per_node: u64,
        socket_addr_space: SocketAddrSpace,
    ) -> Self {
        Self::new(
            &mut ClusterConfig::new_with_equal_stakes(
                num_nodes,
                cluster_lamports,
                lamports_per_node,
            ),
            socket_addr_space,
        )
    }

    fn sync_ledger_path_across_nested_config_fields(
        config: &mut ValidatorConfig,
        ledger_path: &Path,
    ) {
        config.account_paths = vec![
            create_accounts_run_and_snapshot_dirs(ledger_path.join("accounts"))
                .unwrap()
                .0,
        ];
        config.tower_storage = Arc::new(FileTowerStorage::new(ledger_path.to_path_buf()));

        let snapshot_config = &mut config.snapshot_config;
        let dummy: PathBuf = DUMMY_SNAPSHOT_CONFIG_PATH_MARKER.into();
        if snapshot_config.full_snapshot_archives_dir == dummy {
            snapshot_config.full_snapshot_archives_dir = ledger_path.to_path_buf();
        }
        if snapshot_config.bank_snapshots_dir == dummy {
            snapshot_config.bank_snapshots_dir = ledger_path.join("snapshot");
        }
    }

    pub fn new(config: &mut ClusterConfig, socket_addr_space: SocketAddrSpace) -> Self {
        assert_eq!(config.validator_configs.len(), config.node_stakes.len());

        let connection_cache = if config.tpu_use_quic {
            let client_keypair = Keypair::new();
            let stake = DEFAULT_NODE_STAKE;

            for validator_config in config.validator_configs.iter_mut() {
                let mut overrides = HashMap::new();
                overrides.insert(client_keypair.pubkey(), stake);
                validator_config.staked_nodes_overrides = Arc::new(RwLock::new(overrides));
            }

            assert!(
                config.tpu_use_quic,
                "no support for staked override forwarding without quic"
            );

            let total_stake = config.node_stakes.iter().sum::<u64>();
            let stakes = HashMap::from([
                (client_keypair.pubkey(), stake),
                (Pubkey::new_unique(), total_stake.saturating_sub(stake)),
            ]);
            let staked_nodes = Arc::new(RwLock::new(StakedNodes::new(
                Arc::new(stakes),
                HashMap::<Pubkey, u64>::default(), // overrides
            )));

            Arc::new(ConnectionCache::new_with_client_options(
                "connection_cache_local_cluster_quic_staked",
                config.tpu_connection_pool_size,
                None,
                Some((&client_keypair, IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)))),
                Some((&staked_nodes, &client_keypair.pubkey())),
            ))
        } else {
            Arc::new(ConnectionCache::with_udp(
                "connection_cache_local_cluster_udp",
                config.tpu_connection_pool_size,
            ))
        };

        let mut validator_keys = {
            if let Some(ref keys) = config.validator_keys {
                assert_eq!(config.validator_configs.len(), keys.len());
                keys.clone()
            } else {
                iter::repeat_with(|| (Arc::new(Keypair::new()), false))
                    .take(config.validator_configs.len())
                    .collect()
            }
        };

        let vote_keys = {
            if let Some(ref node_vote_keys) = config.node_vote_keys {
                assert_eq!(config.validator_configs.len(), node_vote_keys.len());
                node_vote_keys.clone()
            } else {
                iter::repeat_with(|| Arc::new(Keypair::new()))
                    .take(config.validator_configs.len())
                    .collect()
            }
        };

        // Bootstrap leader should always be in genesis block
        validator_keys[0].1 = true;
        let (keys_in_genesis, stakes_in_genesis): (Vec<ValidatorVoteKeypairs>, Vec<u64>) =
            validator_keys
                .iter()
                .zip(&config.node_stakes)
                .zip(&vote_keys)
                .filter_map(|(((node_keypair, in_genesis), stake), vote_keypair)| {
                    info!(
                        "STARTING LOCAL CLUSTER: key {} vote_key {} has {} stake",
                        node_keypair.pubkey(),
                        vote_keypair.pubkey(),
                        stake
                    );
                    if *in_genesis {
                        Some((
                            ValidatorVoteKeypairs {
                                node_keypair: node_keypair.insecure_clone(),
                                vote_keypair: vote_keypair.insecure_clone(),
                                stake_keypair: Keypair::new(),
                            },
                            stake,
                        ))
                    } else {
                        None
                    }
                })
                .unzip();
        let leader_keypair = &keys_in_genesis[0].node_keypair;
        let leader_vote_keypair = &keys_in_genesis[0].vote_keypair;
        let leader_pubkey = leader_keypair.pubkey();
        let leader_node = Node::new_localhost_with_pubkey(&leader_pubkey);

        let GenesisConfigInfo {
            mut genesis_config,
            mint_keypair,
            ..
        } = create_genesis_config_with_vote_accounts_and_cluster_type(
            config.cluster_lamports,
            &keys_in_genesis,
            stakes_in_genesis,
            config.cluster_type,
        );
        genesis_config.accounts.extend(
            config
                .additional_accounts
                .drain(..)
                .map(|(key, account)| (key, Account::from(account))),
        );
        genesis_config.ticks_per_slot = config.ticks_per_slot;
        genesis_config.epoch_schedule = EpochSchedule::custom(
            config.slots_per_epoch,
            config.stakers_slot_offset,
            !config.skip_warmup_slots,
        );
        genesis_config.poh_config = config.poh_config.clone();
        genesis_config
            .native_instruction_processors
            .extend_from_slice(&config.native_instruction_processors);

        let mut leader_config = safe_clone_config(&config.validator_configs[0]);
        let (leader_ledger_path, _blockhash) = create_new_tmp_ledger_with_size!(
            &genesis_config,
            leader_config.max_genesis_archive_unpacked_size,
        );

        let leader_contact_info = leader_node.info.clone();
        leader_config.rpc_addrs = Some((
            leader_node.info.rpc().unwrap(),
            leader_node.info.rpc_pubsub().unwrap(),
        ));
        Self::sync_ledger_path_across_nested_config_fields(&mut leader_config, &leader_ledger_path);
        let leader_keypair = Arc::new(leader_keypair.insecure_clone());
        let leader_vote_keypair = Arc::new(leader_vote_keypair.insecure_clone());

        let leader_server = Validator::new(
            leader_node,
            leader_keypair.clone(),
            &leader_ledger_path,
            &leader_vote_keypair.pubkey(),
            Arc::new(RwLock::new(vec![leader_vote_keypair.clone()])),
            vec![],
            &leader_config,
            true, // should_check_duplicate_instance
            None, // rpc_to_plugin_manager_receiver
            Arc::new(RwLock::new(ValidatorStartProgress::default())),
            socket_addr_space,
            DEFAULT_TPU_USE_QUIC,
            DEFAULT_TPU_CONNECTION_POOL_SIZE,
            // We are turning tpu_enable_udp to true in order to prevent concurrent local cluster tests
            // to use the same QUIC ports due to SO_REUSEPORT.
            true,
            32, // max connections per IpAddr per minute
            Arc::new(RwLock::new(None)),
        )
        .expect("assume successful validator start");

        let mut validators = HashMap::new();
        let leader_info = ValidatorInfo {
            keypair: leader_keypair,
            voting_keypair: leader_vote_keypair,
            ledger_path: leader_ledger_path,
            contact_info: leader_contact_info.clone(),
        };
        let cluster_leader = ClusterValidatorInfo::new(
            leader_info,
            safe_clone_config(&config.validator_configs[0]),
            leader_server,
        );

        validators.insert(leader_pubkey, cluster_leader);

        let mut cluster = Self {
            funding_keypair: mint_keypair,
            entry_point_info: leader_contact_info,
            validators,
            genesis_config,
            connection_cache,
        };

        let node_pubkey_to_vote_key: HashMap<Pubkey, Arc<Keypair>> = keys_in_genesis
            .into_iter()
            .map(|keypairs| {
                (
                    keypairs.node_keypair.pubkey(),
                    Arc::new(keypairs.vote_keypair.insecure_clone()),
                )
            })
            .collect();
        for (stake, validator_config, (key, _)) in izip!(
            config.node_stakes[1..].iter(),
            config.validator_configs[1..].iter(),
            validator_keys[1..].iter(),
        ) {
            cluster.add_validator(
                validator_config,
                *stake,
                key.clone(),
                node_pubkey_to_vote_key.get(&key.pubkey()).cloned(),
                socket_addr_space,
            );
        }

        let mut listener_config = safe_clone_config(&config.validator_configs[0]);
        listener_config.voting_disabled = true;
        (0..config.num_listeners).for_each(|_| {
            cluster.add_validator_listener(
                &listener_config,
                0,
                Arc::new(Keypair::new()),
                None,
                socket_addr_space,
            );
        });

        discover_cluster(
            &cluster.entry_point_info.gossip().unwrap(),
            config.node_stakes.len() + config.num_listeners as usize,
            socket_addr_space,
        )
        .unwrap();

        discover_cluster(
            &cluster.entry_point_info.gossip().unwrap(),
            config.node_stakes.len(),
            socket_addr_space,
        )
        .unwrap();

        cluster
    }

    pub fn exit(&mut self) {
        for node in self.validators.values_mut() {
            if let Some(ref mut v) = node.validator {
                v.exit();
            }
        }
    }

    pub fn close_preserve_ledgers(&mut self) {
        self.exit();
        for (_, node) in self.validators.iter_mut() {
            if let Some(v) = node.validator.take() {
                v.join();
            }
        }
    }

    /// Set up validator without voting or staking accounts
    pub fn add_validator_listener(
        &mut self,
        validator_config: &ValidatorConfig,
        stake: u64,
        validator_keypair: Arc<Keypair>,
        voting_keypair: Option<Arc<Keypair>>,
        socket_addr_space: SocketAddrSpace,
    ) -> Pubkey {
        self.do_add_validator(
            validator_config,
            true,
            stake,
            validator_keypair,
            voting_keypair,
            socket_addr_space,
        )
    }

    /// Set up validator with voting and staking accounts
    pub fn add_validator(
        &mut self,
        validator_config: &ValidatorConfig,
        stake: u64,
        validator_keypair: Arc<Keypair>,
        voting_keypair: Option<Arc<Keypair>>,
        socket_addr_space: SocketAddrSpace,
    ) -> Pubkey {
        self.do_add_validator(
            validator_config,
            false,
            stake,
            validator_keypair,
            voting_keypair,
            socket_addr_space,
        )
    }

    fn do_add_validator(
        &mut self,
        validator_config: &ValidatorConfig,
        is_listener: bool,
        stake: u64,
        validator_keypair: Arc<Keypair>,
        mut voting_keypair: Option<Arc<Keypair>>,
        socket_addr_space: SocketAddrSpace,
    ) -> Pubkey {
        let client = self.build_tpu_quic_client().expect("tpu_client");

        // Must have enough tokens to fund vote account and set delegate
        let should_create_vote_pubkey = voting_keypair.is_none();
        if voting_keypair.is_none() {
            voting_keypair = Some(Arc::new(Keypair::new()));
        }
        let validator_pubkey = validator_keypair.pubkey();
        let validator_node = Node::new_localhost_with_pubkey(&validator_keypair.pubkey());
        let contact_info = validator_node.info.clone();
        let (ledger_path, _blockhash) = create_new_tmp_ledger_with_size!(
            &self.genesis_config,
            validator_config.max_genesis_archive_unpacked_size,
        );

        // Give the validator some lamports to setup vote accounts
        if is_listener {
            // setup as a listener
            info!("listener {} ", validator_pubkey,);
        } else if should_create_vote_pubkey {
            let validator_balance = Self::transfer_with_client(
                &client,
                &self.funding_keypair,
                &validator_pubkey,
                stake * 2 + 2,
            );
            info!(
                "validator {} balance {}",
                validator_pubkey, validator_balance
            );
            Self::setup_vote_and_stake_accounts(
                &client,
                voting_keypair.as_ref().unwrap(),
                &validator_keypair,
                stake,
            )
            .unwrap();
        }

        let mut config = safe_clone_config(validator_config);
        config.rpc_addrs = Some((
            validator_node.info.rpc().unwrap(),
            validator_node.info.rpc_pubsub().unwrap(),
        ));
        Self::sync_ledger_path_across_nested_config_fields(&mut config, &ledger_path);
        let voting_keypair = voting_keypair.unwrap();
        let validator_server = Validator::new(
            validator_node,
            validator_keypair.clone(),
            &ledger_path,
            &voting_keypair.pubkey(),
            Arc::new(RwLock::new(vec![voting_keypair.clone()])),
            vec![self.entry_point_info.clone()],
            &config,
            true, // should_check_duplicate_instance
            None, // rpc_to_plugin_manager_receiver
            Arc::new(RwLock::new(ValidatorStartProgress::default())),
            socket_addr_space,
            DEFAULT_TPU_USE_QUIC,
            DEFAULT_TPU_CONNECTION_POOL_SIZE,
            DEFAULT_TPU_ENABLE_UDP,
            32, // max connections per IpAddr per mintute
            Arc::new(RwLock::new(None)),
        )
        .expect("assume successful validator start");

        let validator_pubkey = validator_keypair.pubkey();
        let validator_info = ClusterValidatorInfo::new(
            ValidatorInfo {
                keypair: validator_keypair,
                voting_keypair,
                ledger_path,
                contact_info,
            },
            safe_clone_config(validator_config),
            validator_server,
        );

        self.validators.insert(validator_pubkey, validator_info);
        validator_pubkey
    }

    pub fn ledger_path(&self, validator_pubkey: &Pubkey) -> PathBuf {
        self.validators
            .get(validator_pubkey)
            .unwrap()
            .info
            .ledger_path
            .clone()
    }

    fn close(&mut self) {
        self.close_preserve_ledgers();
    }

    pub fn transfer(&self, source_keypair: &Keypair, dest_pubkey: &Pubkey, lamports: u64) -> u64 {
        let client = self.build_tpu_quic_client().expect("new tpu quic client");
        Self::transfer_with_client(&client, source_keypair, dest_pubkey, lamports)
    }

    fn discover_nodes(
        &self,
        socket_addr_space: SocketAddrSpace,
        test_name: &str,
    ) -> Vec<ContactInfo> {
        let alive_node_contact_infos: Vec<_> = self
            .validators
            .values()
            .map(|v| v.info.contact_info.clone())
            .collect();
        assert!(!alive_node_contact_infos.is_empty());
        info!("{} discovering nodes", test_name);
        let cluster_nodes = discover_cluster(
            &alive_node_contact_infos[0].gossip().unwrap(),
            alive_node_contact_infos.len(),
            socket_addr_space,
        )
        .unwrap();
        info!("{} discovered {} nodes", test_name, cluster_nodes.len());
        alive_node_contact_infos
    }

    pub fn check_min_slot_is_rooted(
        &self,
        min_root: Slot,
        test_name: &str,
        socket_addr_space: SocketAddrSpace,
    ) {
        let alive_node_contact_infos = self.discover_nodes(socket_addr_space, test_name);
        info!(
            "{} looking minimum root {} on all nodes",
            test_name, min_root
        );
        cluster_tests::check_min_slot_is_rooted(
            min_root,
            &alive_node_contact_infos,
            &self.connection_cache,
            test_name,
        );
        info!("{} done waiting for roots", test_name);
    }

    pub fn check_for_new_roots(
        &self,
        num_new_roots: usize,
        test_name: &str,
        socket_addr_space: SocketAddrSpace,
    ) {
        let alive_node_contact_infos = self.discover_nodes(socket_addr_space, test_name);
        info!("{} looking for new roots on all nodes", test_name);
        cluster_tests::check_for_new_roots(
            num_new_roots,
            &alive_node_contact_infos,
            &self.connection_cache,
            test_name,
        );
        info!("{} done waiting for roots", test_name);
    }

    pub fn check_no_new_roots(
        &self,
        num_slots_to_wait: usize,
        test_name: &str,
        socket_addr_space: SocketAddrSpace,
    ) {
        let alive_node_contact_infos: Vec<_> = self
            .validators
            .values()
            .map(|node| &node.info.contact_info)
            .collect();
        assert!(!alive_node_contact_infos.is_empty());
        info!("{} discovering nodes", test_name);
        let cluster_nodes = discover_cluster(
            &alive_node_contact_infos[0].gossip().unwrap(),
            alive_node_contact_infos.len(),
            socket_addr_space,
        )
        .unwrap();
        info!("{} discovered {} nodes", test_name, cluster_nodes.len());
        info!("{} making sure no new roots on any nodes", test_name);
        cluster_tests::check_no_new_roots(
            num_slots_to_wait,
            &alive_node_contact_infos,
            &self.connection_cache,
            test_name,
        );
        info!("{} done waiting for roots", test_name);
    }

    /// Attempt to send and confirm tx "attempts" times
    /// Wait for signature confirmation before returning
    /// Return the transaction signature
    pub fn send_transaction_with_retries<T: Signers + ?Sized>(
        client: &QuicTpuClient,
        keypairs: &T,
        transaction: &mut Transaction,
        attempts: usize,
        pending_confirmations: usize,
    ) -> std::result::Result<Signature, TransportError> {
        for attempt in 0..attempts {
            let now = Instant::now();
            let mut num_confirmed = 0;
            let mut wait_time = MAX_PROCESSING_AGE;

            while now.elapsed().as_secs() < wait_time as u64 {
                if num_confirmed == 0 {
                    client.send_transaction_to_upcoming_leaders(transaction)?;
                }

                if let Ok(confirmed_blocks) = client.rpc_client().poll_for_signature_confirmation(
                    &transaction.signatures[0],
                    pending_confirmations,
                ) {
                    num_confirmed = confirmed_blocks;
                    if confirmed_blocks >= pending_confirmations {
                        return Ok(transaction.signatures[0]);
                    }
                    // Since network has seen the transaction, wait longer to receive
                    // all pending confirmations. Resending the transaction could result into
                    // extra transaction fees
                    wait_time = wait_time.max(
                        MAX_PROCESSING_AGE * pending_confirmations.saturating_sub(num_confirmed),
                    );
                }
            }
            info!("{attempt} tries failed transfer");
            let blockhash = client.rpc_client().get_latest_blockhash()?;
            transaction.sign(keypairs, blockhash);
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "failed to confirm transaction".to_string(),
        )
        .into())
    }

    fn transfer_with_client(
        client: &QuicTpuClient,
        source_keypair: &Keypair,
        dest_pubkey: &Pubkey,
        lamports: u64,
    ) -> u64 {
        trace!("getting leader blockhash");
        let (blockhash, _) = client
            .rpc_client()
            .get_latest_blockhash_with_commitment(CommitmentConfig::processed())
            .unwrap();
        let mut tx = system_transaction::transfer(source_keypair, dest_pubkey, lamports, blockhash);
        info!(
            "executing transfer of {} from {} to {}",
            lamports,
            source_keypair.pubkey(),
            *dest_pubkey
        );

        LocalCluster::send_transaction_with_retries(client, &[source_keypair], &mut tx, 10, 0)
            .expect("client transfer should succeed");
        client
            .rpc_client()
            .wait_for_balance_with_commitment(
                dest_pubkey,
                Some(lamports),
                CommitmentConfig::processed(),
            )
            .expect("get balance should succeed")
    }

    fn setup_vote_and_stake_accounts(
        client: &QuicTpuClient,
        vote_account: &Keypair,
        from_account: &Arc<Keypair>,
        amount: u64,
    ) -> Result<()> {
        let vote_account_pubkey = vote_account.pubkey();
        let node_pubkey = from_account.pubkey();
        info!(
            "setup_vote_and_stake_accounts: {}, {}, amount: {}",
            node_pubkey, vote_account_pubkey, amount,
        );
        let stake_account_keypair = Keypair::new();
        let stake_account_pubkey = stake_account_keypair.pubkey();

        // Create the vote account if necessary
        if client
            .rpc_client()
            .poll_get_balance_with_commitment(&vote_account_pubkey, CommitmentConfig::processed())
            .unwrap_or(0)
            == 0
        {
            // 1) Create vote account
            // Unlike the bootstrap validator we have to check if the new vote state is being used
            // as the cluster is already running, and using the wrong account size will cause the
            // InitializeAccount tx to fail
            let use_current_vote_state = client
                .rpc_client()
                .poll_get_balance_with_commitment(
                    &feature_set::vote_state_add_vote_latency::id(),
                    CommitmentConfig::processed(),
                )
                .unwrap_or(0)
                > 0;

            let instructions = vote_instruction::create_account_with_config(
                &from_account.pubkey(),
                &vote_account_pubkey,
                &VoteInit {
                    node_pubkey,
                    authorized_voter: vote_account_pubkey,
                    authorized_withdrawer: vote_account_pubkey,
                    commission: 0,
                },
                amount,
                vote_instruction::CreateVoteAccountConfig {
                    space: vote_state::VoteStateVersions::vote_state_size_of(use_current_vote_state)
                        as u64,
                    ..vote_instruction::CreateVoteAccountConfig::default()
                },
            );
            let message = Message::new(&instructions, Some(&from_account.pubkey()));
            let mut transaction = Transaction::new(
                &[from_account.as_ref(), vote_account],
                message,
                client
                    .rpc_client()
                    .get_latest_blockhash_with_commitment(CommitmentConfig::processed())
                    .unwrap()
                    .0,
            );
            LocalCluster::send_transaction_with_retries(
                client,
                &[from_account],
                &mut transaction,
                10,
                0,
            )
            .expect("should fund vote");
            client
                .rpc_client()
                .wait_for_balance_with_commitment(
                    &vote_account_pubkey,
                    Some(amount),
                    CommitmentConfig::processed(),
                )
                .expect("get balance");

            let instructions = stake_instruction::create_account_and_delegate_stake(
                &from_account.pubkey(),
                &stake_account_pubkey,
                &vote_account_pubkey,
                &Authorized::auto(&stake_account_pubkey),
                &Lockup::default(),
                amount,
            );
            let message = Message::new(&instructions, Some(&from_account.pubkey()));
            let mut transaction = Transaction::new(
                &[from_account.as_ref(), &stake_account_keypair],
                message,
                client
                    .rpc_client()
                    .get_latest_blockhash_with_commitment(CommitmentConfig::processed())
                    .unwrap()
                    .0,
            );

            LocalCluster::send_transaction_with_retries(
                client,
                &[from_account.as_ref(), &stake_account_keypair],
                &mut transaction,
                5,
                0,
            )
            .expect("should delegate stake");
            client
                .rpc_client()
                .wait_for_balance_with_commitment(
                    &stake_account_pubkey,
                    Some(amount),
                    CommitmentConfig::processed(),
                )
                .expect("get balance");
        } else {
            warn!(
                "{} vote_account already has a balance?!?",
                vote_account_pubkey
            );
        }
        info!("Checking for vote account registration of {}", node_pubkey);
        match (
            client
                .rpc_client()
                .get_account_with_commitment(&stake_account_pubkey, CommitmentConfig::processed()),
            client
                .rpc_client()
                .get_account_with_commitment(&vote_account_pubkey, CommitmentConfig::processed()),
        ) {
            (Ok(stake_account), Ok(vote_account)) => {
                match (stake_account.value, vote_account.value) {
                    (Some(stake_account), Some(vote_account)) => {
                        match (
                            stake_state::stake_from(&stake_account),
                            vote_state::from(&vote_account),
                        ) {
                            (Some(stake_state), Some(vote_state)) => {
                                if stake_state.delegation.voter_pubkey != vote_account_pubkey
                                    || stake_state.delegation.stake != amount
                                {
                                    Err(Error::new(ErrorKind::Other, "invalid stake account state"))
                                } else if vote_state.node_pubkey != node_pubkey {
                                    Err(Error::new(ErrorKind::Other, "invalid vote account state"))
                                } else {
                                    info!(
                                        "node {} {:?} {:?}",
                                        node_pubkey, stake_state, vote_state
                                    );

                                    return Ok(());
                                }
                            }
                            (None, _) => {
                                Err(Error::new(ErrorKind::Other, "invalid stake account data"))
                            }
                            (_, None) => {
                                Err(Error::new(ErrorKind::Other, "invalid vote account data"))
                            }
                        }
                    }
                    (None, _) => Err(Error::new(
                        ErrorKind::Other,
                        "unable to retrieve stake account data",
                    )),
                    (_, None) => Err(Error::new(
                        ErrorKind::Other,
                        "unable to retrieve vote account data",
                    )),
                }
            }
            (Err(_), _) => Err(Error::new(
                ErrorKind::Other,
                "unable to retrieve stake account data",
            )),
            (_, Err(_)) => Err(Error::new(
                ErrorKind::Other,
                "unable to retrieve vote account data",
            )),
        }
    }

    pub fn create_dummy_load_only_snapshot_config() -> SnapshotConfig {
        // DUMMY_SNAPSHOT_CONFIG_PATH_MARKER will be replaced with real value as part of cluster
        // node lifecycle.
        // There must be some place holder for now...
        SnapshotConfig {
            full_snapshot_archives_dir: DUMMY_SNAPSHOT_CONFIG_PATH_MARKER.into(),
            bank_snapshots_dir: DUMMY_SNAPSHOT_CONFIG_PATH_MARKER.into(),
            ..SnapshotConfig::new_load_only()
        }
    }

    fn build_tpu_client<F>(&self, rpc_client_builder: F) -> Result<QuicTpuClient>
    where
        F: FnOnce(String) -> Arc<RpcClient>,
    {
        let rpc_pubsub_url = format!("ws://{}/", self.entry_point_info.rpc_pubsub().unwrap());
        let rpc_url = format!("http://{}", self.entry_point_info.rpc().unwrap());

        let cache = match &*self.connection_cache {
            ConnectionCache::Quic(cache) => cache,
            ConnectionCache::Udp(_) => {
                return Err(Error::new(
                    ErrorKind::Other,
                    "Expected a Quic ConnectionCache. Got UDP",
                ))
            }
        };

        let tpu_client = TpuClient::new_with_connection_cache(
            rpc_client_builder(rpc_url),
            rpc_pubsub_url.as_str(),
            TpuClientConfig::default(),
            cache.clone(),
        )
        .map_err(|err| Error::new(ErrorKind::Other, format!("TpuSenderError: {}", err)))?;

        Ok(tpu_client)
    }
}

impl Cluster for LocalCluster {
    fn get_node_pubkeys(&self) -> Vec<Pubkey> {
        self.validators.keys().cloned().collect()
    }

    fn get_validator_client(&self, pubkey: &Pubkey) -> Option<QuicTpuClient> {
        self.validators.get(pubkey).map(|_| {
            self.build_tpu_quic_client()
                .expect("should build tpu quic client")
        })
    }

    fn build_tpu_quic_client(&self) -> Result<QuicTpuClient> {
        self.build_tpu_client(|rpc_url| Arc::new(RpcClient::new(rpc_url)))
    }

    fn build_tpu_quic_client_with_commitment(
        &self,
        commitment_config: CommitmentConfig,
    ) -> Result<QuicTpuClient> {
        self.build_tpu_client(|rpc_url| {
            Arc::new(RpcClient::new_with_commitment(rpc_url, commitment_config))
        })
    }

    fn exit_node(&mut self, pubkey: &Pubkey) -> ClusterValidatorInfo {
        let mut node = self.validators.remove(pubkey).unwrap();

        // Shut down the validator
        let mut validator = node.validator.take().expect("Validator must be running");
        validator.exit();
        validator.join();
        node
    }

    fn create_restart_context(
        &mut self,
        pubkey: &Pubkey,
        cluster_validator_info: &mut ClusterValidatorInfo,
    ) -> (Node, Vec<ContactInfo>) {
        // Update the stored ContactInfo for this node
        let node = Node::new_localhost_with_pubkey(pubkey);
        cluster_validator_info.info.contact_info = node.info.clone();
        cluster_validator_info.config.rpc_addrs =
            Some((node.info.rpc().unwrap(), node.info.rpc_pubsub().unwrap()));

        if pubkey == self.entry_point_info.pubkey() {
            self.entry_point_info = node.info.clone();
        }

        let mut is_entrypoint_alive = false;
        let mut entry_point_infos: Vec<ContactInfo> = self
            .validators
            .values()
            .map(|validator| {
                // Should not be restarting a validator that is still alive
                assert!(validator.info.contact_info.pubkey() != pubkey);
                if validator.info.contact_info.pubkey() == self.entry_point_info.pubkey() {
                    is_entrypoint_alive = true;
                }
                validator.info.contact_info.clone()
            })
            .collect();

        if !is_entrypoint_alive {
            entry_point_infos.push(self.entry_point_info.clone());
        }

        (node, entry_point_infos)
    }

    fn set_entry_point(&mut self, entry_point_info: ContactInfo) {
        self.entry_point_info = entry_point_info;
    }

    fn restart_node(
        &mut self,
        pubkey: &Pubkey,
        mut cluster_validator_info: ClusterValidatorInfo,
        socket_addr_space: SocketAddrSpace,
    ) {
        let restart_context = self.create_restart_context(pubkey, &mut cluster_validator_info);
        let cluster_validator_info = Self::restart_node_with_context(
            cluster_validator_info,
            restart_context,
            socket_addr_space,
        );
        self.add_node(pubkey, cluster_validator_info);
    }

    fn add_node(&mut self, pubkey: &Pubkey, cluster_validator_info: ClusterValidatorInfo) {
        self.validators.insert(*pubkey, cluster_validator_info);
    }

    fn restart_node_with_context(
        mut cluster_validator_info: ClusterValidatorInfo,
        (node, entry_point_infos): (Node, Vec<ContactInfo>),
        socket_addr_space: SocketAddrSpace,
    ) -> ClusterValidatorInfo {
        // Restart the node
        let validator_info = &cluster_validator_info.info;
        LocalCluster::sync_ledger_path_across_nested_config_fields(
            &mut cluster_validator_info.config,
            &validator_info.ledger_path,
        );
        let restarted_node = Validator::new(
            node,
            validator_info.keypair.clone(),
            &validator_info.ledger_path,
            &validator_info.voting_keypair.pubkey(),
            Arc::new(RwLock::new(vec![validator_info.voting_keypair.clone()])),
            entry_point_infos,
            &safe_clone_config(&cluster_validator_info.config),
            true, // should_check_duplicate_instance
            None, // rpc_to_plugin_manager_receiver
            Arc::new(RwLock::new(ValidatorStartProgress::default())),
            socket_addr_space,
            DEFAULT_TPU_USE_QUIC,
            DEFAULT_TPU_CONNECTION_POOL_SIZE,
            DEFAULT_TPU_ENABLE_UDP,
            32, // max connections per IpAddr per minute, use higher value because of tests
            Arc::new(RwLock::new(None)),
        )
        .expect("assume successful validator start");
        cluster_validator_info.validator = Some(restarted_node);
        cluster_validator_info
    }

    fn exit_restart_node(
        &mut self,
        pubkey: &Pubkey,
        validator_config: ValidatorConfig,
        socket_addr_space: SocketAddrSpace,
    ) {
        let mut cluster_validator_info = self.exit_node(pubkey);
        cluster_validator_info.config = validator_config;
        self.restart_node(pubkey, cluster_validator_info, socket_addr_space);
    }

    fn get_contact_info(&self, pubkey: &Pubkey) -> Option<&ContactInfo> {
        self.validators.get(pubkey).map(|v| &v.info.contact_info)
    }

    fn send_shreds_to_validator(&self, dup_shreds: Vec<&Shred>, validator_key: &Pubkey) {
        let send_socket = UdpSocket::bind("0.0.0.0:0").unwrap();
        let validator_tvu = self
            .get_contact_info(validator_key)
            .unwrap()
            .tvu(Protocol::UDP)
            .unwrap();
        for shred in dup_shreds {
            send_socket
                .send_to(shred.payload().as_ref(), validator_tvu)
                .unwrap();
        }
    }
}

impl Drop for LocalCluster {
    fn drop(&mut self) {
        self.close();
    }
}
