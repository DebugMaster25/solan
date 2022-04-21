use {
    crate::tpu_info::TpuInfo,
    crossbeam_channel::{Receiver, RecvTimeoutError},
    log::*,
    solana_client::connection_cache,
    solana_measure::measure::Measure,
    solana_metrics::{datapoint_warn, inc_new_counter_info},
    solana_runtime::{bank::Bank, bank_forks::BankForks},
    solana_sdk::{hash::Hash, nonce_account, pubkey::Pubkey, signature::Signature},
    std::{
        collections::{
            hash_map::{Entry, HashMap},
            HashSet,
        },
        net::SocketAddr,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex, RwLock,
        },
        thread::{self, sleep, Builder, JoinHandle},
        time::{Duration, Instant},
    },
};

/// Maximum size of the transaction queue
const MAX_TRANSACTION_QUEUE_SIZE: usize = 10_000; // This seems like a lot but maybe it needs to be bigger one day

/// Default retry interval
const DEFAULT_RETRY_RATE_MS: u64 = 2_000;

/// Default number of leaders to forward transactions to
const DEFAULT_LEADER_FORWARD_COUNT: u64 = 2;
/// Default max number of time the service will retry broadcast
const DEFAULT_SERVICE_MAX_RETRIES: usize = usize::MAX;

/// Default batch size for sending transaction in batch
/// When this size is reached, send out the transactions.
const DEFAULT_TRANSACTION_BATCH_SIZE: usize = 1;

// The maximum transaction batch size
pub const MAX_TRANSACTION_BATCH_SIZE: usize = 10_000;

/// Maximum transaction sends per second
pub const MAX_TRANSACTION_SENDS_PER_SECOND: u64 = 1_000;

/// Default maximum batch waiting time in ms. If this time is reached,
/// whatever transactions are cached will be sent.
const DEFAULT_BATCH_SEND_RATE_MS: u64 = 1;

// The maximum transaction batch send rate in MS
pub const MAX_BATCH_SEND_RATE_MS: usize = 100_000;

pub struct SendTransactionService {
    receive_txn_thread: JoinHandle<()>,
    retry_thread: JoinHandle<()>,
    exit: Arc<AtomicBool>,
}

pub struct TransactionInfo {
    pub signature: Signature,
    pub wire_transaction: Vec<u8>,
    pub last_valid_block_height: u64,
    pub durable_nonce_info: Option<(Pubkey, Hash)>,
    pub max_retries: Option<usize>,
    retries: usize,
    /// Last time the transaction was sent
    last_sent_time: Option<Instant>,
}

impl TransactionInfo {
    pub fn new(
        signature: Signature,
        wire_transaction: Vec<u8>,
        last_valid_block_height: u64,
        durable_nonce_info: Option<(Pubkey, Hash)>,
        max_retries: Option<usize>,
        last_sent_time: Option<Instant>,
    ) -> Self {
        Self {
            signature,
            wire_transaction,
            last_valid_block_height,
            durable_nonce_info,
            max_retries,
            retries: 0,
            last_sent_time,
        }
    }
}

#[derive(Default, Debug, PartialEq)]
struct ProcessTransactionsResult {
    rooted: u64,
    expired: u64,
    retried: u64,
    max_retries_elapsed: u64,
    failed: u64,
    retained: u64,
}

pub const DEFAULT_TPU_USE_QUIC: bool = false;

#[derive(Clone, Debug)]
pub struct Config {
    pub retry_rate_ms: u64,
    pub leader_forward_count: u64,
    pub default_max_retries: Option<usize>,
    pub service_max_retries: usize,
    /// Whether to use Quic protocol to send transactions
    pub use_quic: bool,
    /// The batch size for sending transactions in batches
    pub batch_size: usize,
    /// How frequently batches are sent
    pub batch_send_rate_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            retry_rate_ms: DEFAULT_RETRY_RATE_MS,
            leader_forward_count: DEFAULT_LEADER_FORWARD_COUNT,
            default_max_retries: None,
            service_max_retries: DEFAULT_SERVICE_MAX_RETRIES,
            use_quic: DEFAULT_TPU_USE_QUIC,
            batch_size: DEFAULT_TRANSACTION_BATCH_SIZE,
            batch_send_rate_ms: DEFAULT_BATCH_SEND_RATE_MS,
        }
    }
}

impl SendTransactionService {
    pub fn new<T: TpuInfo + std::marker::Send + Clone + 'static>(
        tpu_address: SocketAddr,
        bank_forks: &Arc<RwLock<BankForks>>,
        leader_info: Option<T>,
        receiver: Receiver<TransactionInfo>,
        retry_rate_ms: u64,
        leader_forward_count: u64,
        use_quic: bool,
    ) -> Self {
        let config = Config {
            retry_rate_ms,
            leader_forward_count,
            use_quic,
            ..Config::default()
        };
        Self::new_with_config(tpu_address, bank_forks, leader_info, receiver, config)
    }

    pub fn new_with_config<T: TpuInfo + std::marker::Send + Clone + 'static>(
        tpu_address: SocketAddr,
        bank_forks: &Arc<RwLock<BankForks>>,
        leader_info: Option<T>,
        receiver: Receiver<TransactionInfo>,
        config: Config,
    ) -> Self {
        let retry_transactions = Arc::new(Mutex::new(HashMap::new()));
        let exit = Arc::new(AtomicBool::new(false));
        let receive_txn_thread = Self::receive_txn_thread(
            tpu_address,
            receiver,
            leader_info.clone(),
            config.clone(),
            retry_transactions.clone(),
            exit.clone(),
        );

        let retry_thread = Self::retry_thread(
            tpu_address,
            bank_forks.clone(),
            leader_info,
            config,
            retry_transactions,
            exit.clone(),
        );
        Self {
            receive_txn_thread,
            retry_thread,
            exit,
        }
    }

    /// Thread responsible for receiving transactions from RPC clients.
    fn receive_txn_thread<T: TpuInfo + std::marker::Send + 'static>(
        tpu_address: SocketAddr,
        receiver: Receiver<TransactionInfo>,
        mut leader_info: Option<T>,
        config: Config,
        retry_transactions: Arc<Mutex<HashMap<Signature, TransactionInfo>>>,
        exit: Arc<AtomicBool>,
    ) -> JoinHandle<()> {
        let mut last_batch_sent = Instant::now();
        let mut last_leader_refresh = Instant::now();
        let mut transactions = HashMap::new();

        info!(
            "Starting send-transaction-service::receive_txn_thread with config {:?}",
            config
        );
        if let Some(leader_info) = leader_info.as_mut() {
            leader_info.refresh_recent_peers();
        }
        connection_cache::set_use_quic(config.use_quic);
        Builder::new()
            .name("send-tx-receive".to_string())
            .spawn(move || loop {
                let recv_timeout_ms = config.batch_send_rate_ms;
                match receiver.recv_timeout(Duration::from_millis(1000.min(recv_timeout_ms))) {
                    Err(RecvTimeoutError::Disconnected) => {
                        info!("Terminating send-transaction-service.");
                        exit.store(true, Ordering::Relaxed);
                        break;
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Ok(transaction_info) => {
                        inc_new_counter_info!("send_transaction_service-recv-tx", 1);
                        let entry = transactions.entry(transaction_info.signature);
                        let mut new_transaction = false;
                        if let Entry::Vacant(_) = entry {
                            if !retry_transactions
                                .lock()
                                .unwrap()
                                .contains_key(&transaction_info.signature)
                            {
                                entry.or_insert(transaction_info);
                                new_transaction = true;
                            }
                        }
                        if !new_transaction {
                            inc_new_counter_info!("send_transaction_service-recv-duplicate", 1);
                        }
                    }
                }

                if (!transactions.is_empty()
                    && last_batch_sent.elapsed().as_millis() as u64 >= config.batch_send_rate_ms)
                    || transactions.len() >= config.batch_size
                {
                    inc_new_counter_info!(
                        "send_transaction_service-batch-size",
                        transactions.len()
                    );
                    let _result = Self::send_transactions_in_batch(
                        &tpu_address,
                        &mut transactions,
                        &leader_info,
                        &config,
                    );
                    let last_sent_time = Instant::now();
                    {
                        // take a lock of retry_transactions and move the batch to the retry set.
                        let mut retry_transactions = retry_transactions.lock().unwrap();
                        for (signature, mut transaction_info) in transactions.drain() {
                            let retry_len = retry_transactions.len();
                            let entry = retry_transactions.entry(signature);
                            if let Entry::Vacant(_) = entry {
                                if retry_len >= MAX_TRANSACTION_QUEUE_SIZE {
                                    datapoint_warn!("send_transaction_service-queue-overflow");
                                    break;
                                } else {
                                    transaction_info.last_sent_time = Some(last_sent_time);
                                    entry.or_insert(transaction_info);
                                }
                            }
                        }
                    }

                    last_batch_sent = Instant::now();
                    if last_leader_refresh.elapsed().as_millis() > 1000 {
                        if let Some(leader_info) = leader_info.as_mut() {
                            leader_info.refresh_recent_peers();
                        }
                        last_leader_refresh = Instant::now();
                    }
                }
            })
            .unwrap()
    }

    /// Thread responsible for retrying transactions
    fn retry_thread<T: TpuInfo + std::marker::Send + 'static>(
        tpu_address: SocketAddr,
        bank_forks: Arc<RwLock<BankForks>>,
        mut leader_info: Option<T>,
        config: Config,
        retry_transactions: Arc<Mutex<HashMap<Signature, TransactionInfo>>>,
        exit: Arc<AtomicBool>,
    ) -> JoinHandle<()> {
        let mut last_leader_refresh = Instant::now();

        info!(
            "Starting send-transaction-service::retry_thread with config {:?}",
            config
        );
        if let Some(leader_info) = leader_info.as_mut() {
            leader_info.refresh_recent_peers();
        }
        connection_cache::set_use_quic(config.use_quic);
        Builder::new()
            .name("send-tx-retry".to_string())
            .spawn(move || loop {
                let retry_interval_ms = config.retry_rate_ms;
                sleep(Duration::from_millis(1000.min(retry_interval_ms)));
                if exit.load(Ordering::Relaxed) {
                    break;
                }
                let mut transactions = retry_transactions.lock().unwrap();
                if !transactions.is_empty() {
                    datapoint_info!(
                        "send_transaction_service-queue-size",
                        ("len", transactions.len(), i64)
                    );
                    let (root_bank, working_bank) = {
                        let bank_forks = bank_forks.read().unwrap();
                        (
                            bank_forks.root_bank().clone(),
                            bank_forks.working_bank().clone(),
                        )
                    };

                    let _result = Self::process_transactions(
                        &working_bank,
                        &root_bank,
                        &tpu_address,
                        &mut transactions,
                        &leader_info,
                        &config,
                    );
                }
                if last_leader_refresh.elapsed().as_millis() > 1000 {
                    if let Some(leader_info) = leader_info.as_mut() {
                        leader_info.refresh_recent_peers();
                    }
                    last_leader_refresh = Instant::now();
                }
            })
            .unwrap()
    }

    /// Process transactions in batch.
    fn send_transactions_in_batch<T: TpuInfo>(
        tpu_address: &SocketAddr,
        transactions: &mut HashMap<Signature, TransactionInfo>,
        leader_info: &Option<T>,
        config: &Config,
    ) {
        let mut measure = Measure::start("send_transactions_in_batch-us");

        // Processing the transactions in batch
        let addresses = Self::get_tpu_addresses(tpu_address, leader_info, config);

        let wire_transactions = transactions
            .iter()
            .map(|(_, transaction_info)| transaction_info.wire_transaction.as_ref())
            .collect::<Vec<&[u8]>>();

        for address in &addresses {
            Self::send_transactions(address, &wire_transactions);
        }
        measure.stop();
        inc_new_counter_info!(
            "send_transactions_in_batch-us",
            measure.as_us() as usize,
            1000,
            1000
        );
    }

    /// Retry transactions sent before.
    fn process_transactions<T: TpuInfo>(
        working_bank: &Arc<Bank>,
        root_bank: &Arc<Bank>,
        tpu_address: &SocketAddr,
        transactions: &mut HashMap<Signature, TransactionInfo>,
        leader_info: &Option<T>,
        config: &Config,
    ) -> ProcessTransactionsResult {
        let mut result = ProcessTransactionsResult::default();

        let mut batched_transactions = HashSet::new();
        let retry_rate = Duration::from_millis(config.retry_rate_ms);

        transactions.retain(|signature, mut transaction_info| {
            if transaction_info.durable_nonce_info.is_some() {
                inc_new_counter_info!("send_transaction_service-nonced", 1);
            }
            if root_bank.has_signature(signature) {
                info!("Transaction is rooted: {}", signature);
                result.rooted += 1;
                inc_new_counter_info!("send_transaction_service-rooted", 1);
                return false;
            }
            let signature_status = working_bank.get_signature_status_slot(signature);
            if let Some((nonce_pubkey, durable_nonce)) = transaction_info.durable_nonce_info {
                let nonce_account = working_bank.get_account(&nonce_pubkey).unwrap_or_default();
                let now = Instant::now();
                let expired = transaction_info
                    .last_sent_time
                    .map(|last| now.duration_since(last) >= retry_rate)
                    .unwrap_or(false);
                if !nonce_account::verify_nonce_account(&nonce_account, &durable_nonce)
                    && signature_status.is_none()
                    && expired
                {
                    info!("Dropping expired durable-nonce transaction: {}", signature);
                    result.expired += 1;
                    inc_new_counter_info!("send_transaction_service-expired", 1);
                    return false;
                }
            }
            if transaction_info.last_valid_block_height < root_bank.block_height() {
                info!("Dropping expired transaction: {}", signature);
                result.expired += 1;
                inc_new_counter_info!("send_transaction_service-expired", 1);
                return false;
            }

            let max_retries = transaction_info
                .max_retries
                .or(config.default_max_retries)
                .map(|max_retries| max_retries.min(config.service_max_retries));

            if let Some(max_retries) = max_retries {
                if transaction_info.retries >= max_retries {
                    info!("Dropping transaction due to max retries: {}", signature);
                    result.max_retries_elapsed += 1;
                    inc_new_counter_info!("send_transaction_service-max_retries", 1);
                    return false;
                }
            }

            match signature_status {
                None => {
                    let now = Instant::now();
                    let need_send = transaction_info
                        .last_sent_time
                        .map(|last| now.duration_since(last) >= retry_rate)
                        .unwrap_or(true);
                    if need_send {
                        if transaction_info.last_sent_time.is_some() {
                            // Transaction sent before is unknown to the working bank, it might have been
                            // dropped or landed in another fork.  Re-send it

                            info!("Retrying transaction: {}", signature);
                            result.retried += 1;
                            transaction_info.retries += 1;

                            inc_new_counter_info!("send_transaction_service-retry", 1);
                        }

                        batched_transactions.insert(*signature);
                        transaction_info.last_sent_time = Some(now);
                    }
                    true
                }
                Some((_slot, status)) => {
                    if status.is_err() {
                        info!("Dropping failed transaction: {}", signature);
                        result.failed += 1;
                        inc_new_counter_info!("send_transaction_service-failed", 1);
                        false
                    } else {
                        result.retained += 1;
                        true
                    }
                }
            }
        });

        if !batched_transactions.is_empty() {
            // Processing the transactions in batch
            let addresses = Self::get_tpu_addresses(tpu_address, leader_info, config);

            let wire_transactions = transactions
                .iter()
                .filter(|(signature, _)| batched_transactions.contains(signature))
                .map(|(_, transaction_info)| transaction_info.wire_transaction.as_ref())
                .collect::<Vec<&[u8]>>();

            for address in &addresses {
                let iter = wire_transactions.chunks(config.batch_size);
                for chunk in iter {
                    Self::send_transactions(address, chunk);
                }
            }
        }
        result
    }

    fn send_transaction(tpu_address: &SocketAddr, wire_transaction: &[u8]) {
        let mut measure = Measure::start("send_transaction_service-us");
        if let Err(err) =
            connection_cache::send_wire_transaction_async(wire_transaction.to_vec(), tpu_address)
        {
            warn!("Failed to send transaction to {}: {:?}", tpu_address, err);
        }
        measure.stop();
        inc_new_counter_info!(
            "send_transaction_service-us",
            measure.as_us() as usize,
            1000,
            1000
        );
    }

    fn send_transactions_with_metrics(tpu_address: &SocketAddr, wire_transactions: &[&[u8]]) {
        let mut measure = Measure::start("send_transaction_service-batch-us");

        let wire_transactions = wire_transactions.iter().map(|t| t.to_vec()).collect();
        let send_result =
            connection_cache::send_wire_transaction_batch_async(wire_transactions, tpu_address);
        if let Err(err) = send_result {
            warn!(
                "Failed to send transaction batch to {}: {:?}",
                tpu_address, err
            );
        }
        measure.stop();
        inc_new_counter_info!(
            "send_transaction_service-batch-us",
            measure.as_us() as usize
        );
    }

    fn send_transactions(tpu_address: &SocketAddr, wire_transactions: &[&[u8]]) {
        if wire_transactions.len() == 1 {
            Self::send_transaction(tpu_address, wire_transactions[0])
        } else {
            Self::send_transactions_with_metrics(tpu_address, wire_transactions)
        }
    }

    fn get_tpu_addresses<'a, T: TpuInfo>(
        tpu_address: &'a SocketAddr,
        leader_info: &'a Option<T>,
        config: &'a Config,
    ) -> Vec<&'a SocketAddr> {
        let addresses = leader_info
            .as_ref()
            .map(|leader_info| leader_info.get_leader_tpus(config.leader_forward_count));
        addresses
            .map(|address_list| {
                if address_list.is_empty() {
                    vec![tpu_address]
                } else {
                    address_list
                }
            })
            .unwrap_or_else(|| vec![tpu_address])
    }

    pub fn join(self) -> thread::Result<()> {
        self.receive_txn_thread.join()?;
        self.exit.store(true, Ordering::Relaxed);
        self.retry_thread.join()
    }
}

#[cfg(test)]
mod test {
    use {
        super::*,
        crate::tpu_info::NullTpuInfo,
        crossbeam_channel::unbounded,
        solana_sdk::{
            account::AccountSharedData, genesis_config::create_genesis_config, nonce,
            pubkey::Pubkey, signature::Signer, system_program, system_transaction,
        },
        std::ops::Sub,
    };

    #[test]
    fn service_exit() {
        let tpu_address = "127.0.0.1:0".parse().unwrap();
        let bank = Bank::default_for_tests();
        let bank_forks = Arc::new(RwLock::new(BankForks::new(bank)));
        let (sender, receiver) = unbounded();

        let send_tranaction_service = SendTransactionService::new::<NullTpuInfo>(
            tpu_address,
            &bank_forks,
            None,
            receiver,
            1000,
            1,
            DEFAULT_TPU_USE_QUIC,
        );

        drop(sender);
        send_tranaction_service.join().unwrap();
    }

    #[test]
    fn process_transactions() {
        solana_logger::setup();

        let (genesis_config, mint_keypair) = create_genesis_config(4);
        let bank = Bank::new_for_tests(&genesis_config);
        let bank_forks = Arc::new(RwLock::new(BankForks::new(bank)));
        let tpu_address = "127.0.0.1:0".parse().unwrap();
        let config = Config {
            leader_forward_count: 1,
            ..Config::default()
        };

        let root_bank = Arc::new(Bank::new_from_parent(
            &bank_forks.read().unwrap().working_bank(),
            &Pubkey::default(),
            1,
        ));
        let rooted_signature = root_bank
            .transfer(1, &mint_keypair, &mint_keypair.pubkey())
            .unwrap();

        let working_bank = Arc::new(Bank::new_from_parent(&root_bank, &Pubkey::default(), 2));

        let non_rooted_signature = working_bank
            .transfer(2, &mint_keypair, &mint_keypair.pubkey())
            .unwrap();

        let failed_signature = {
            let blockhash = working_bank.last_blockhash();
            let transaction =
                system_transaction::transfer(&mint_keypair, &Pubkey::default(), 1, blockhash);
            let signature = transaction.signatures[0];
            working_bank.process_transaction(&transaction).unwrap_err();
            signature
        };

        let mut transactions = HashMap::new();

        info!("Expired transactions are dropped...");
        transactions.insert(
            Signature::default(),
            TransactionInfo::new(
                Signature::default(),
                vec![],
                root_bank.block_height() - 1,
                None,
                None,
                Some(Instant::now()),
            ),
        );
        let result = SendTransactionService::process_transactions::<NullTpuInfo>(
            &working_bank,
            &root_bank,
            &tpu_address,
            &mut transactions,
            &None,
            &config,
        );
        assert!(transactions.is_empty());
        assert_eq!(
            result,
            ProcessTransactionsResult {
                expired: 1,
                ..ProcessTransactionsResult::default()
            }
        );

        info!("Rooted transactions are dropped...");
        transactions.insert(
            rooted_signature,
            TransactionInfo::new(
                rooted_signature,
                vec![],
                working_bank.block_height(),
                None,
                None,
                Some(Instant::now()),
            ),
        );
        let result = SendTransactionService::process_transactions::<NullTpuInfo>(
            &working_bank,
            &root_bank,
            &tpu_address,
            &mut transactions,
            &None,
            &config,
        );
        assert!(transactions.is_empty());
        assert_eq!(
            result,
            ProcessTransactionsResult {
                rooted: 1,
                ..ProcessTransactionsResult::default()
            }
        );

        info!("Failed transactions are dropped...");
        transactions.insert(
            failed_signature,
            TransactionInfo::new(
                failed_signature,
                vec![],
                working_bank.block_height(),
                None,
                None,
                Some(Instant::now()),
            ),
        );
        let result = SendTransactionService::process_transactions::<NullTpuInfo>(
            &working_bank,
            &root_bank,
            &tpu_address,
            &mut transactions,
            &None,
            &config,
        );
        assert!(transactions.is_empty());
        assert_eq!(
            result,
            ProcessTransactionsResult {
                failed: 1,
                ..ProcessTransactionsResult::default()
            }
        );

        info!("Non-rooted transactions are kept...");
        transactions.insert(
            non_rooted_signature,
            TransactionInfo::new(
                non_rooted_signature,
                vec![],
                working_bank.block_height(),
                None,
                None,
                Some(Instant::now()),
            ),
        );
        let result = SendTransactionService::process_transactions::<NullTpuInfo>(
            &working_bank,
            &root_bank,
            &tpu_address,
            &mut transactions,
            &None,
            &config,
        );
        assert_eq!(transactions.len(), 1);
        assert_eq!(
            result,
            ProcessTransactionsResult {
                retained: 1,
                ..ProcessTransactionsResult::default()
            }
        );
        transactions.clear();

        info!("Unknown transactions are retried...");
        transactions.insert(
            Signature::default(),
            TransactionInfo::new(
                Signature::default(),
                vec![],
                working_bank.block_height(),
                None,
                None,
                Some(Instant::now().sub(Duration::from_millis(4000))),
            ),
        );

        let result = SendTransactionService::process_transactions::<NullTpuInfo>(
            &working_bank,
            &root_bank,
            &tpu_address,
            &mut transactions,
            &None,
            &config,
        );
        assert_eq!(transactions.len(), 1);
        assert_eq!(
            result,
            ProcessTransactionsResult {
                retried: 1,
                ..ProcessTransactionsResult::default()
            }
        );
        transactions.clear();

        info!("Transactions are only retried until max_retries");
        transactions.insert(
            Signature::new(&[1; 64]),
            TransactionInfo::new(
                Signature::default(),
                vec![],
                working_bank.block_height(),
                None,
                Some(0),
                Some(Instant::now()),
            ),
        );
        transactions.insert(
            Signature::new(&[2; 64]),
            TransactionInfo::new(
                Signature::default(),
                vec![],
                working_bank.block_height(),
                None,
                Some(1),
                Some(Instant::now().sub(Duration::from_millis(4000))),
            ),
        );
        let result = SendTransactionService::process_transactions::<NullTpuInfo>(
            &working_bank,
            &root_bank,
            &tpu_address,
            &mut transactions,
            &None,
            &config,
        );
        assert_eq!(transactions.len(), 1);
        assert_eq!(
            result,
            ProcessTransactionsResult {
                retried: 1,
                max_retries_elapsed: 1,
                ..ProcessTransactionsResult::default()
            }
        );
        let result = SendTransactionService::process_transactions::<NullTpuInfo>(
            &working_bank,
            &root_bank,
            &tpu_address,
            &mut transactions,
            &None,
            &config,
        );
        assert!(transactions.is_empty());
        assert_eq!(
            result,
            ProcessTransactionsResult {
                max_retries_elapsed: 1,
                ..ProcessTransactionsResult::default()
            }
        );
    }

    #[test]
    fn test_retry_durable_nonce_transactions() {
        solana_logger::setup();

        let (genesis_config, mint_keypair) = create_genesis_config(4);
        let bank = Bank::new_for_tests(&genesis_config);
        let bank_forks = Arc::new(RwLock::new(BankForks::new(bank)));
        let tpu_address = "127.0.0.1:0".parse().unwrap();
        let config = Config {
            leader_forward_count: 1,
            ..Config::default()
        };

        let root_bank = Arc::new(Bank::new_from_parent(
            &bank_forks.read().unwrap().working_bank(),
            &Pubkey::default(),
            1,
        ));
        let rooted_signature = root_bank
            .transfer(1, &mint_keypair, &mint_keypair.pubkey())
            .unwrap();

        let nonce_address = Pubkey::new_unique();
        let durable_nonce = Hash::new_unique();
        let nonce_state = nonce::state::Versions::new_current(nonce::State::Initialized(
            nonce::state::Data::new(Pubkey::default(), durable_nonce, 42),
        ));
        let nonce_account =
            AccountSharedData::new_data(43, &nonce_state, &system_program::id()).unwrap();
        root_bank.store_account(&nonce_address, &nonce_account);

        let working_bank = Arc::new(Bank::new_from_parent(&root_bank, &Pubkey::default(), 2));
        let non_rooted_signature = working_bank
            .transfer(2, &mint_keypair, &mint_keypair.pubkey())
            .unwrap();

        let last_valid_block_height = working_bank.block_height() + 300;

        let failed_signature = {
            let blockhash = working_bank.last_blockhash();
            let transaction =
                system_transaction::transfer(&mint_keypair, &Pubkey::default(), 1, blockhash);
            let signature = transaction.signatures[0];
            working_bank.process_transaction(&transaction).unwrap_err();
            signature
        };

        let mut transactions = HashMap::new();

        info!("Rooted durable-nonce transactions are dropped...");
        transactions.insert(
            rooted_signature,
            TransactionInfo::new(
                rooted_signature,
                vec![],
                last_valid_block_height,
                Some((nonce_address, durable_nonce)),
                None,
                Some(Instant::now()),
            ),
        );
        let result = SendTransactionService::process_transactions::<NullTpuInfo>(
            &working_bank,
            &root_bank,
            &tpu_address,
            &mut transactions,
            &None,
            &config,
        );
        assert!(transactions.is_empty());
        assert_eq!(
            result,
            ProcessTransactionsResult {
                rooted: 1,
                ..ProcessTransactionsResult::default()
            }
        );
        // Nonce expired case
        transactions.insert(
            rooted_signature,
            TransactionInfo::new(
                rooted_signature,
                vec![],
                last_valid_block_height,
                Some((nonce_address, Hash::new_unique())),
                None,
                Some(Instant::now()),
            ),
        );
        let result = SendTransactionService::process_transactions::<NullTpuInfo>(
            &working_bank,
            &root_bank,
            &tpu_address,
            &mut transactions,
            &None,
            &config,
        );
        assert!(transactions.is_empty());
        assert_eq!(
            result,
            ProcessTransactionsResult {
                rooted: 1,
                ..ProcessTransactionsResult::default()
            }
        );

        // Expired durable-nonce transactions are dropped; nonce has advanced...
        info!("Expired durable-nonce transactions are dropped...");
        transactions.insert(
            Signature::default(),
            TransactionInfo::new(
                Signature::default(),
                vec![],
                last_valid_block_height,
                Some((nonce_address, Hash::new_unique())),
                None,
                Some(Instant::now().sub(Duration::from_millis(4000))),
            ),
        );
        let result = SendTransactionService::process_transactions::<NullTpuInfo>(
            &working_bank,
            &root_bank,
            &tpu_address,
            &mut transactions,
            &None,
            &config,
        );
        assert!(transactions.is_empty());
        assert_eq!(
            result,
            ProcessTransactionsResult {
                expired: 1,
                ..ProcessTransactionsResult::default()
            }
        );
        // ... or last_valid_block_height timeout has passed
        transactions.insert(
            Signature::default(),
            TransactionInfo::new(
                Signature::default(),
                vec![],
                root_bank.block_height() - 1,
                Some((nonce_address, durable_nonce)),
                None,
                Some(Instant::now()),
            ),
        );
        let result = SendTransactionService::process_transactions::<NullTpuInfo>(
            &working_bank,
            &root_bank,
            &tpu_address,
            &mut transactions,
            &None,
            &config,
        );
        assert!(transactions.is_empty());
        assert_eq!(
            result,
            ProcessTransactionsResult {
                expired: 1,
                ..ProcessTransactionsResult::default()
            }
        );

        info!("Failed durable-nonce transactions are dropped...");
        transactions.insert(
            failed_signature,
            TransactionInfo::new(
                failed_signature,
                vec![],
                last_valid_block_height,
                Some((nonce_address, Hash::new_unique())), // runtime should advance nonce on failed transactions
                None,
                Some(Instant::now()),
            ),
        );
        let result = SendTransactionService::process_transactions::<NullTpuInfo>(
            &working_bank,
            &root_bank,
            &tpu_address,
            &mut transactions,
            &None,
            &config,
        );
        assert!(transactions.is_empty());
        assert_eq!(
            result,
            ProcessTransactionsResult {
                failed: 1,
                ..ProcessTransactionsResult::default()
            }
        );

        info!("Non-rooted durable-nonce transactions are kept...");
        transactions.insert(
            non_rooted_signature,
            TransactionInfo::new(
                non_rooted_signature,
                vec![],
                last_valid_block_height,
                Some((nonce_address, Hash::new_unique())), // runtime advances nonce when transaction lands
                None,
                Some(Instant::now()),
            ),
        );
        let result = SendTransactionService::process_transactions::<NullTpuInfo>(
            &working_bank,
            &root_bank,
            &tpu_address,
            &mut transactions,
            &None,
            &config,
        );
        assert_eq!(transactions.len(), 1);
        assert_eq!(
            result,
            ProcessTransactionsResult {
                retained: 1,
                ..ProcessTransactionsResult::default()
            }
        );
        transactions.clear();

        info!("Unknown durable-nonce transactions are retried until nonce advances...");
        // simulate there was a nonce transaction sent 4 seconds ago (> the retry rate which is 2 seconds)
        transactions.insert(
            Signature::default(),
            TransactionInfo::new(
                Signature::default(),
                vec![],
                last_valid_block_height,
                Some((nonce_address, durable_nonce)),
                None,
                Some(Instant::now().sub(Duration::from_millis(4000))),
            ),
        );
        let result = SendTransactionService::process_transactions::<NullTpuInfo>(
            &working_bank,
            &root_bank,
            &tpu_address,
            &mut transactions,
            &None,
            &config,
        );
        assert_eq!(transactions.len(), 1);
        assert_eq!(
            result,
            ProcessTransactionsResult {
                retried: 1,
                ..ProcessTransactionsResult::default()
            }
        );
        // Advance nonce, simulate the transaction was again last sent 4 seconds ago.
        // This time the transaction should have been dropped.
        for mut transaction in transactions.values_mut() {
            transaction.last_sent_time = Some(Instant::now().sub(Duration::from_millis(4000)));
        }
        let new_durable_nonce = Hash::new_unique();
        let new_nonce_state = nonce::state::Versions::new_current(nonce::State::Initialized(
            nonce::state::Data::new(Pubkey::default(), new_durable_nonce, 42),
        ));
        let nonce_account =
            AccountSharedData::new_data(43, &new_nonce_state, &system_program::id()).unwrap();
        working_bank.store_account(&nonce_address, &nonce_account);
        let result = SendTransactionService::process_transactions::<NullTpuInfo>(
            &working_bank,
            &root_bank,
            &tpu_address,
            &mut transactions,
            &None,
            &config,
        );
        assert_eq!(transactions.len(), 0);
        assert_eq!(
            result,
            ProcessTransactionsResult {
                expired: 1,
                ..ProcessTransactionsResult::default()
            }
        );
    }
}
