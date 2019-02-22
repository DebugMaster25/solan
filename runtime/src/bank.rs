//! The `bank` module tracks client accounts and the progress of on-chain
//! programs. It offers a high-level API that signs transactions
//! on behalf of the caller, and a low-level API for when they have
//! already been signed and verified.

use crate::accounts::{Accounts, ErrorCounters, InstructionAccounts, InstructionLoaders};
use crate::last_id_queue::LastIdQueue;
use crate::runtime::{self, RuntimeError};
use crate::status_cache::StatusCache;
use bincode::{deserialize, serialize};
use hashbrown::HashMap;
use log::{debug, info, Level};
use solana_metrics::counter::Counter;
use solana_sdk::account::Account;
use solana_sdk::bpf_loader;
use solana_sdk::budget_program;
use solana_sdk::genesis_block::GenesisBlock;
use solana_sdk::hash::{extend_and_hash, Hash};
use solana_sdk::native_loader;
use solana_sdk::native_program::ProgramError;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature};
use solana_sdk::storage_program;
use solana_sdk::system_program;
use solana_sdk::system_transaction::SystemTransaction;
use solana_sdk::timing::{duration_as_us, MAX_ENTRY_IDS, NUM_TICKS_PER_SECOND};
use solana_sdk::token_program;
use solana_sdk::transaction::Transaction;
use solana_sdk::vote_program::{self, VoteState};
use std::result;
use std::sync::{Arc, RwLock};
use std::time::Instant;

/// Reasons a transaction might be rejected.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum BankError {
    /// This Pubkey is being processed in another transaction
    AccountInUse,

    /// Pubkey appears twice in the same transaction, typically in a pay-to-self
    /// transaction.
    AccountLoadedTwice,

    /// Attempt to debit from `Pubkey`, but no found no record of a prior credit.
    AccountNotFound,

    /// The from `Pubkey` does not have sufficient balance to pay the fee to schedule the transaction
    InsufficientFundsForFee,

    /// The bank has seen `Signature` before. This can occur under normal operation
    /// when a UDP packet is duplicated, as a user error from a client not updating
    /// its `last_id`, or as a double-spend attack.
    DuplicateSignature,

    /// The bank has not seen the given `last_id` or the transaction is too old and
    /// the `last_id` has been discarded.
    LastIdNotFound,

    /// Proof of History verification failed.
    LedgerVerificationFailed,

    /// The program returned an error
    ProgramError(u8, ProgramError),

    /// Recoding into PoH failed
    RecordFailure,

    /// Loader call chain too deep
    CallChainTooDeep,

    /// Transaction has a fee but has no signature present
    MissingSignatureForFee,
}

pub type Result<T> = result::Result<T, BankError>;

type BankStatusCache = StatusCache<BankError>;

/// Manager for the state of all accounts and programs after processing its entries.
#[derive(Default)]
pub struct Bank {
    accounts: Accounts,

    /// A cache of signature statuses
    status_cache: RwLock<BankStatusCache>,

    /// FIFO queue of `last_id` items
    last_id_queue: RwLock<LastIdQueue>,

    /// Previous checkpoint of this bank
    parent: Option<Arc<Bank>>,

    /// Hash of this Bank's state. Only meaningful after freezing it via `new_from_parent()`.
    hash: RwLock<Hash>,

    /// The number of ticks in each slot.
    ticks_per_slot: u64,

    /// The number of slots in each epoch.
    slots_per_epoch: u64,

    // A number of slots before slot_index 0. Used to generate the current
    // epoch's leader schedule.
    leader_schedule_slot_offset: u64,

    /// Slot leader
    leader: Pubkey,
}

impl Bank {
    pub fn new(genesis_block: &GenesisBlock) -> Self {
        let mut bank = Self::default();
        bank.process_genesis_block(genesis_block);
        bank.add_builtin_programs();
        bank
    }

    /// Create a new bank that points to an immutable checkpoint of another bank.
    pub fn new_from_parent(parent: &Arc<Bank>, leader: &Pubkey) -> Self {
        let mut bank = Self::default();
        bank.last_id_queue = RwLock::new(parent.last_id_queue.read().unwrap().clone());
        bank.ticks_per_slot = parent.ticks_per_slot;
        bank.slots_per_epoch = parent.slots_per_epoch;
        bank.leader_schedule_slot_offset = parent.leader_schedule_slot_offset;

        bank.parent = Some(parent.clone());
        if *parent.hash.read().unwrap() == Hash::default() {
            *parent.hash.write().unwrap() = parent.hash_internal_state();
        }
        bank.leader = *leader;
        bank
    }

    /// merge (i.e. pull) the parent's state up into this Bank,
    ///   this Bank becomes a root
    pub fn merge_parents(&mut self) {
        let parents = self.parents();
        self.parent = None;

        let parent_accounts: Vec<_> = parents.iter().map(|b| &b.accounts).collect();
        self.accounts.merge_parents(&parent_accounts);

        let parent_caches: Vec<_> = parents
            .iter()
            .map(|b| b.status_cache.read().unwrap())
            .collect();
        self.status_cache
            .write()
            .unwrap()
            .merge_parents(&parent_caches);
    }

    /// Return the more recent checkpoint of this bank instance.
    pub fn parent(&self) -> Option<Arc<Bank>> {
        self.parent.clone()
    }
    /// Returns whether this bank is the root
    pub fn is_root(&self) -> bool {
        self.parent.is_none()
    }

    fn process_genesis_block(&mut self, genesis_block: &GenesisBlock) {
        assert!(genesis_block.mint_id != Pubkey::default());
        assert!(genesis_block.bootstrap_leader_id != Pubkey::default());
        assert!(genesis_block.bootstrap_leader_vote_account_id != Pubkey::default());
        assert!(genesis_block.tokens >= genesis_block.bootstrap_leader_tokens);
        assert!(genesis_block.bootstrap_leader_tokens >= 2);

        let mint_tokens = genesis_block.tokens - genesis_block.bootstrap_leader_tokens;
        self.deposit(&genesis_block.mint_id, mint_tokens);

        let bootstrap_leader_tokens = genesis_block.bootstrap_leader_tokens - 1;
        self.deposit(&genesis_block.bootstrap_leader_id, bootstrap_leader_tokens);

        // Construct a vote account for the bootstrap_leader such that the leader_scheduler
        // will be forced to select it as the leader for height 0
        let mut bootstrap_leader_vote_account = Account {
            tokens: 1,
            userdata: vec![0; vote_program::get_max_size() as usize],
            owner: vote_program::id(),
            executable: false,
        };

        let mut vote_state = VoteState::new(
            genesis_block.bootstrap_leader_id,
            genesis_block.bootstrap_leader_id,
        );
        vote_state.votes.push_back(vote_program::Vote::new(0));
        vote_state
            .serialize(&mut bootstrap_leader_vote_account.userdata)
            .unwrap();

        self.accounts.store_slow(
            self.is_root(),
            &genesis_block.bootstrap_leader_vote_account_id,
            &bootstrap_leader_vote_account,
        );

        self.last_id_queue
            .write()
            .unwrap()
            .genesis_last_id(&genesis_block.last_id());

        self.ticks_per_slot = genesis_block.ticks_per_slot;
        self.slots_per_epoch = genesis_block.slots_per_epoch;
        self.leader_schedule_slot_offset = genesis_block.leader_schedule_slot_offset;
    }

    pub fn add_native_program(&self, name: &str, program_id: &Pubkey) {
        let account = native_loader::create_program_account(name);
        self.accounts
            .store_slow(self.is_root(), program_id, &account);
    }

    fn add_builtin_programs(&self) {
        self.add_native_program("solana_system_program", &system_program::id());
        self.add_native_program("solana_vote_program", &vote_program::id());
        self.add_native_program("solana_storage_program", &storage_program::id());
        self.add_native_program("solana_bpf_loader", &bpf_loader::id());
        self.add_native_program("solana_budget_program", &budget_program::id());
        self.add_native_program("solana_erc20", &token_program::id());

        let storage_system_account = Account::new(1, 16 * 1024, storage_program::system_id());
        self.accounts.store_slow(
            self.is_root(),
            &storage_program::system_id(),
            &storage_system_account,
        );
    }

    /// Return the last entry ID registered.
    pub fn last_id(&self) -> Hash {
        self.last_id_queue
            .read()
            .unwrap()
            .last_id
            .expect("no last_id has been set")
    }

    pub fn get_storage_entry_height(&self) -> u64 {
        match self.get_account(&storage_program::system_id()) {
            Some(storage_system_account) => {
                let state = deserialize(&storage_system_account.userdata);
                if let Ok(state) = state {
                    let state: storage_program::StorageProgramState = state;
                    return state.entry_height;
                }
            }
            None => {
                info!("error in reading entry_height");
            }
        }
        0
    }

    pub fn get_storage_last_id(&self) -> Hash {
        if let Some(storage_system_account) = self.get_account(&storage_program::system_id()) {
            let state = deserialize(&storage_system_account.userdata);
            if let Ok(state) = state {
                let state: storage_program::StorageProgramState = state;
                return state.id;
            }
        }
        Hash::default()
    }

    /// Forget all signatures. Useful for benchmarking.
    pub fn clear_signatures(&self) {
        self.status_cache.write().unwrap().clear();
    }

    fn update_transaction_statuses(&self, txs: &[Transaction], res: &[Result<()>]) {
        let mut status_cache = self.status_cache.write().unwrap();
        for (i, tx) in txs.iter().enumerate() {
            match &res[i] {
                Ok(_) => status_cache.add(&tx.signatures[0]),
                Err(BankError::LastIdNotFound) => (),
                Err(BankError::DuplicateSignature) => (),
                Err(BankError::AccountNotFound) => (),
                Err(e) => {
                    status_cache.add(&tx.signatures[0]);
                    status_cache.save_failure_status(&tx.signatures[0], e.clone());
                }
            }
        }
    }

    /// Looks through a list of tick heights and stakes, and finds the latest
    /// tick that has achieved confirmation
    pub fn get_confirmation_timestamp(
        &self,
        ticks_and_stakes: &mut [(u64, u64)],
        supermajority_stake: u64,
    ) -> Option<u64> {
        let last_ids = self.last_id_queue.read().unwrap();
        last_ids.get_confirmation_timestamp(ticks_and_stakes, supermajority_stake)
    }

    /// Tell the bank which Entry IDs exist on the ledger. This function
    /// assumes subsequent calls correspond to later entries, and will boot
    /// the oldest ones once its internal cache is full. Once boot, the
    /// bank will reject transactions using that `last_id`.
    pub fn register_tick(&self, last_id: &Hash) {
        let current_tick_height = {
            //atomic register and read the tick
            let mut last_id_queue = self.last_id_queue.write().unwrap();
            inc_new_counter_info!("bank-register_tick-registered", 1);
            last_id_queue.register_tick(last_id);
            last_id_queue.tick_height
        };
        if current_tick_height % NUM_TICKS_PER_SECOND as u64 == 0 {
            self.status_cache.write().unwrap().new_cache(last_id);
        }
    }

    /// Process a Transaction. This is used for unit tests and simply calls the vector Bank::process_transactions method.
    pub fn process_transaction(&self, tx: &Transaction) -> Result<()> {
        let txs = vec![tx.clone()];
        match self.process_transactions(&txs)[0] {
            Err(ref e) => {
                info!("process_transaction error: {:?}", e);
                Err((*e).clone())
            }
            Ok(_) => Ok(()),
        }
    }

    pub fn lock_accounts(&self, txs: &[Transaction]) -> Vec<Result<()>> {
        self.accounts.lock_accounts(txs)
    }

    pub fn unlock_accounts(&self, txs: &[Transaction], results: &[Result<()>]) {
        self.accounts.unlock_accounts(txs, results)
    }

    fn load_accounts(
        &self,
        txs: &[Transaction],
        results: Vec<Result<()>>,
        error_counters: &mut ErrorCounters,
    ) -> Vec<Result<(InstructionAccounts, InstructionLoaders)>> {
        let parents = self.parents();
        let mut accounts = vec![&self.accounts];
        accounts.extend(parents.iter().map(|b| &b.accounts));
        Accounts::load_accounts(&accounts, txs, results, error_counters)
    }
    fn check_age(
        &self,
        txs: &[Transaction],
        lock_results: Vec<Result<()>>,
        max_age: usize,
        error_counters: &mut ErrorCounters,
    ) -> Vec<Result<()>> {
        let last_ids = self.last_id_queue.read().unwrap();
        txs.iter()
            .zip(lock_results.into_iter())
            .map(|(tx, lock_res)| {
                if lock_res.is_ok() && !last_ids.check_entry_id_age(tx.last_id, max_age) {
                    error_counters.reserve_last_id += 1;
                    Err(BankError::LastIdNotFound)
                } else {
                    lock_res
                }
            })
            .collect()
    }
    fn check_signatures(
        &self,
        txs: &[Transaction],
        lock_results: Vec<Result<()>>,
        error_counters: &mut ErrorCounters,
    ) -> Vec<Result<()>> {
        let parents = self.parents();
        let mut caches = vec![self.status_cache.read().unwrap()];
        caches.extend(parents.iter().map(|b| b.status_cache.read().unwrap()));
        txs.iter()
            .zip(lock_results.into_iter())
            .map(|(tx, lock_res)| {
                if lock_res.is_ok() && StatusCache::has_signature_all(&caches, &tx.signatures[0]) {
                    error_counters.duplicate_signature += 1;
                    Err(BankError::DuplicateSignature)
                } else {
                    lock_res
                }
            })
            .collect()
    }
    #[allow(clippy::type_complexity)]
    pub fn load_and_execute_transactions(
        &self,
        txs: &[Transaction],
        lock_results: Vec<Result<()>>,
        max_age: usize,
    ) -> (
        Vec<Result<(InstructionAccounts, InstructionLoaders)>>,
        Vec<Result<()>>,
    ) {
        debug!("processing transactions: {}", txs.len());
        let mut error_counters = ErrorCounters::default();
        let now = Instant::now();
        let age_results = self.check_age(txs, lock_results, max_age, &mut error_counters);
        let sig_results = self.check_signatures(txs, age_results, &mut error_counters);
        let mut loaded_accounts = self.load_accounts(txs, sig_results, &mut error_counters);
        let tick_height = self.tick_height();

        let load_elapsed = now.elapsed();
        let now = Instant::now();
        let executed: Vec<Result<()>> = loaded_accounts
            .iter_mut()
            .zip(txs.iter())
            .map(|(accs, tx)| match accs {
                Err(e) => Err(e.clone()),
                Ok((ref mut accounts, ref mut loaders)) => {
                    runtime::execute_transaction(tx, loaders, accounts, tick_height).map_err(
                        |RuntimeError::ProgramError(index, err)| {
                            BankError::ProgramError(index, err)
                        },
                    )
                }
            })
            .collect();

        let execution_elapsed = now.elapsed();

        debug!(
            "load: {}us execute: {}us txs_len={}",
            duration_as_us(&load_elapsed),
            duration_as_us(&execution_elapsed),
            txs.len(),
        );
        let mut tx_count = 0;
        let mut err_count = 0;
        for (r, tx) in executed.iter().zip(txs.iter()) {
            if r.is_ok() {
                tx_count += 1;
            } else {
                if err_count == 0 {
                    info!("tx error: {:?} {:?}", r, tx);
                }
                err_count += 1;
            }
        }
        if err_count > 0 {
            info!("{} errors of {} txs", err_count, err_count + tx_count);
            inc_new_counter_info!(
                "bank-process_transactions-account_not_found",
                error_counters.account_not_found
            );
            inc_new_counter_info!("bank-process_transactions-error_count", err_count);
        }

        self.accounts.increment_transaction_count(tx_count);

        inc_new_counter_info!("bank-process_transactions-txs", tx_count);
        if 0 != error_counters.last_id_not_found {
            inc_new_counter_info!(
                "bank-process_transactions-error-last_id_not_found",
                error_counters.last_id_not_found
            );
        }
        if 0 != error_counters.reserve_last_id {
            inc_new_counter_info!(
                "bank-process_transactions-error-reserve_last_id",
                error_counters.reserve_last_id
            );
        }
        if 0 != error_counters.duplicate_signature {
            inc_new_counter_info!(
                "bank-process_transactions-error-duplicate_signature",
                error_counters.duplicate_signature
            );
        }
        if 0 != error_counters.insufficient_funds {
            inc_new_counter_info!(
                "bank-process_transactions-error-insufficient_funds",
                error_counters.insufficient_funds
            );
        }
        if 0 != error_counters.account_loaded_twice {
            inc_new_counter_info!(
                "bank-process_transactions-account_loaded_twice",
                error_counters.account_loaded_twice
            );
        }
        (loaded_accounts, executed)
    }

    fn filter_program_errors_and_collect_fee(
        &self,
        txs: &[Transaction],
        executed: &[Result<()>],
    ) -> Vec<Result<()>> {
        let mut fees = 0;
        let results = txs
            .iter()
            .zip(executed.iter())
            .map(|(tx, res)| match *res {
                Err(BankError::ProgramError(_, _)) => {
                    // Charge the transaction fee even in case of ProgramError
                    self.withdraw(&tx.account_keys[0], tx.fee)?;
                    fees += tx.fee;
                    Ok(())
                }
                Ok(()) => {
                    fees += tx.fee;
                    Ok(())
                }
                _ => res.clone(),
            })
            .collect();
        self.deposit(&self.leader, fees);
        results
    }

    pub fn commit_transactions(
        &self,
        txs: &[Transaction],
        loaded_accounts: &[Result<(InstructionAccounts, InstructionLoaders)>],
        executed: &[Result<()>],
    ) -> Vec<Result<()>> {
        let now = Instant::now();
        self.accounts
            .store_accounts(self.is_root(), txs, executed, loaded_accounts);

        // once committed there is no way to unroll
        let write_elapsed = now.elapsed();
        debug!(
            "store: {}us txs_len={}",
            duration_as_us(&write_elapsed),
            txs.len(),
        );
        self.update_transaction_statuses(txs, &executed);
        self.filter_program_errors_and_collect_fee(txs, executed)
    }

    /// Process a batch of transactions.
    #[must_use]
    pub fn load_execute_and_commit_transactions(
        &self,
        txs: &[Transaction],
        lock_results: Vec<Result<()>>,
        max_age: usize,
    ) -> Vec<Result<()>> {
        let (loaded_accounts, executed) =
            self.load_and_execute_transactions(txs, lock_results, max_age);

        self.commit_transactions(txs, &loaded_accounts, &executed)
    }

    #[must_use]
    pub fn process_transactions(&self, txs: &[Transaction]) -> Vec<Result<()>> {
        let lock_results = self.lock_accounts(txs);
        let results = self.load_execute_and_commit_transactions(txs, lock_results, MAX_ENTRY_IDS);
        self.unlock_accounts(txs, &results);
        results
    }

    /// Create, sign, and process a Transaction from `keypair` to `to` of
    /// `n` tokens where `last_id` is the last Entry ID observed by the client.
    pub fn transfer(
        &self,
        n: u64,
        keypair: &Keypair,
        to: Pubkey,
        last_id: Hash,
    ) -> Result<Signature> {
        let tx = SystemTransaction::new_account(keypair, to, n, last_id, 0);
        let signature = tx.signatures[0];
        self.process_transaction(&tx).map(|_| signature)
    }

    pub fn read_balance(account: &Account) -> u64 {
        // TODO: Re-instate budget_program special case?
        /*
        if budget_program::check_id(&account.owner) {
           return budget_program::get_balance(account);
        }
        */
        account.tokens
    }
    /// Each program would need to be able to introspect its own state
    /// this is hard-coded to the Budget language
    pub fn get_balance(&self, pubkey: &Pubkey) -> u64 {
        self.get_account(pubkey)
            .map(|x| Self::read_balance(&x))
            .unwrap_or(0)
    }

    /// Compute all the parents of the bank in order
    fn parents(&self) -> Vec<Arc<Bank>> {
        let mut parents = vec![];
        let mut bank = self.parent();
        while let Some(parent) = bank {
            parents.push(parent.clone());
            bank = parent.parent();
        }
        parents
    }

    pub fn withdraw(&self, pubkey: &Pubkey, tokens: u64) -> Result<()> {
        match self.get_account(pubkey) {
            Some(mut account) => {
                if tokens > account.tokens {
                    return Err(BankError::InsufficientFundsForFee);
                }

                account.tokens -= tokens;
                self.accounts.store_slow(true, pubkey, &account);
                Ok(())
            }
            None => Err(BankError::AccountNotFound),
        }
    }

    pub fn deposit(&self, pubkey: &Pubkey, tokens: u64) {
        let mut account = self.get_account(pubkey).unwrap_or_default();
        account.tokens += tokens;
        self.accounts.store_slow(self.is_root(), pubkey, &account);
    }

    pub fn get_account(&self, pubkey: &Pubkey) -> Option<Account> {
        let parents = self.parents();
        let mut accounts = vec![&self.accounts];
        accounts.extend(parents.iter().map(|b| &b.accounts));
        Accounts::load_slow(&accounts, pubkey)
    }

    pub fn get_account_modified_since_parent(&self, pubkey: &Pubkey) -> Option<Account> {
        Accounts::load_slow(&[&self.accounts], pubkey)
    }

    pub fn transaction_count(&self) -> u64 {
        self.accounts.transaction_count()
    }

    pub fn get_signature_status(&self, signature: &Signature) -> Option<Result<()>> {
        let parents = self.parents();
        let mut caches = vec![self.status_cache.read().unwrap()];
        caches.extend(parents.iter().map(|b| b.status_cache.read().unwrap()));
        StatusCache::get_signature_status_all(&caches, signature)
    }

    pub fn has_signature(&self, signature: &Signature) -> bool {
        let parents = self.parents();
        let mut caches = vec![self.status_cache.read().unwrap()];
        caches.extend(parents.iter().map(|b| b.status_cache.read().unwrap()));
        StatusCache::has_signature_all(&caches, signature)
    }

    /// Hash the `accounts` HashMap. This represents a validator's interpretation
    ///  of the delta of the ledger since the last vote and up to now
    pub fn hash_internal_state(&self) -> Hash {
        // If there are no accounts, return the same hash as we did before
        // checkpointing.
        let accounts = &self.accounts.accounts_db.read().unwrap().accounts;
        let parent_hash = match &self.parent {
            None => Hash::default(),
            Some(parent) => *parent.hash.read().unwrap(),
        };
        if accounts.is_empty() {
            return parent_hash;
        }

        let accounts_delta_hash = self.accounts.hash_internal_state();
        extend_and_hash(&parent_hash, &serialize(&accounts_delta_hash).unwrap())
    }

    pub fn vote_states<F>(&self, cond: F) -> Vec<VoteState>
    where
        F: Fn(&VoteState) -> bool,
    {
        self.accounts
            .accounts_db
            .read()
            .unwrap()
            .accounts
            .values()
            .filter_map(|account| {
                if vote_program::check_id(&account.owner) {
                    if let Ok(vote_state) = VoteState::deserialize(&account.userdata) {
                        if cond(&vote_state) {
                            return Some(vote_state);
                        }
                    }
                }
                None
            })
            .collect()
    }

    /// Collect the node Pubkey and staker account balance for nodes
    /// that have non-zero balance in their corresponding staker accounts
    pub fn staked_nodes(&self) -> HashMap<Pubkey, u64> {
        self.vote_states(|state| self.get_balance(&state.staker_id) > 0)
            .iter()
            .map(|state| (state.node_id, self.get_balance(&state.staker_id)))
            .collect()
    }

    /// Return the number of ticks per slot that should be used calls to slot_height().
    pub fn ticks_per_slot(&self) -> u64 {
        self.ticks_per_slot
    }

    /// Return the number of slots per tick that should be used calls to epoch_height().
    pub fn slots_per_epoch(&self) -> u64 {
        self.slots_per_epoch
    }

    /// Return the checkpointed bank that should be used to generate a leader schedule.
    /// Return None if a sufficiently old bank checkpoint doesn't exist.
    pub fn leader_schedule_bank(&self) -> Option<Arc<Bank>> {
        let epoch_slot_height = self.slot_height() - self.slot_index();
        let expected = epoch_slot_height.saturating_sub(self.leader_schedule_slot_offset);
        self.parents()
            .into_iter()
            .find(|bank| bank.slot_height() <= expected)
    }

    /// Return the number of ticks since genesis.
    pub fn tick_height(&self) -> u64 {
        self.last_id_queue.read().unwrap().tick_height
    }

    /// Return the number of ticks since the last slot boundary.
    pub fn tick_index(&self) -> u64 {
        self.tick_height() % self.ticks_per_slot()
    }

    /// Return the slot_height of the last registered tick.
    pub fn slot_height(&self) -> u64 {
        self.tick_height() / self.ticks_per_slot()
    }

    /// Return the number of slots since the last epoch boundary.
    pub fn slot_index(&self) -> u64 {
        self.slot_height() % self.slots_per_epoch()
    }

    /// Return the epoch height of the last registered tick.
    pub fn epoch_height(&self) -> u64 {
        self.slot_height() / self.slots_per_epoch()
    }

    #[cfg(test)]
    pub fn last_ids(&self) -> &RwLock<LastIdQueue> {
        &self.last_id_queue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hashbrown::HashSet;
    use solana_sdk::genesis_block::BOOTSTRAP_LEADER_TOKENS;
    use solana_sdk::native_program::ProgramError;
    use solana_sdk::signature::{Keypair, KeypairUtil};
    use solana_sdk::system_instruction::SystemInstruction;
    use solana_sdk::system_transaction::SystemTransaction;
    use solana_sdk::transaction::Instruction;

    #[test]
    fn test_bank_new() {
        let (genesis_block, _) = GenesisBlock::new(10_000);
        let bank = Bank::new(&genesis_block);
        assert_eq!(bank.get_balance(&genesis_block.mint_id), 10_000);
    }

    #[test]
    fn test_bank_new_with_leader() {
        let dummy_leader_id = Keypair::new().pubkey();
        let dummy_leader_tokens = BOOTSTRAP_LEADER_TOKENS;
        let (genesis_block, _) =
            GenesisBlock::new_with_leader(10_000, dummy_leader_id, dummy_leader_tokens);
        assert_eq!(genesis_block.bootstrap_leader_tokens, dummy_leader_tokens);
        let bank = Bank::new(&genesis_block);
        assert_eq!(
            bank.get_balance(&genesis_block.mint_id),
            10_000 - dummy_leader_tokens
        );
        assert_eq!(
            bank.get_balance(&dummy_leader_id),
            dummy_leader_tokens - 1 /* 1 token goes to the vote account associated with dummy_leader_tokens */
        );
    }

    #[test]
    fn test_two_payments_to_one_party() {
        let (genesis_block, mint_keypair) = GenesisBlock::new(10_000);
        let pubkey = Keypair::new().pubkey();
        let bank = Bank::new(&genesis_block);
        assert_eq!(bank.last_id(), genesis_block.last_id());

        bank.transfer(1_000, &mint_keypair, pubkey, genesis_block.last_id())
            .unwrap();
        assert_eq!(bank.get_balance(&pubkey), 1_000);

        bank.transfer(500, &mint_keypair, pubkey, genesis_block.last_id())
            .unwrap();
        assert_eq!(bank.get_balance(&pubkey), 1_500);
        assert_eq!(bank.transaction_count(), 2);
    }

    #[test]
    fn test_one_source_two_tx_one_batch() {
        let (genesis_block, mint_keypair) = GenesisBlock::new(1);
        let key1 = Keypair::new().pubkey();
        let key2 = Keypair::new().pubkey();
        let bank = Bank::new(&genesis_block);
        assert_eq!(bank.last_id(), genesis_block.last_id());

        let t1 = SystemTransaction::new_move(&mint_keypair, key1, 1, genesis_block.last_id(), 0);
        let t2 = SystemTransaction::new_move(&mint_keypair, key2, 1, genesis_block.last_id(), 0);
        let res = bank.process_transactions(&vec![t1.clone(), t2.clone()]);
        assert_eq!(res.len(), 2);
        assert_eq!(res[0], Ok(()));
        assert_eq!(res[1], Err(BankError::AccountInUse));
        assert_eq!(bank.get_balance(&mint_keypair.pubkey()), 0);
        assert_eq!(bank.get_balance(&key1), 1);
        assert_eq!(bank.get_balance(&key2), 0);
        assert_eq!(bank.get_signature_status(&t1.signatures[0]), Some(Ok(())));
        // TODO: Transactions that fail to pay a fee could be dropped silently
        assert_eq!(
            bank.get_signature_status(&t2.signatures[0]),
            Some(Err(BankError::AccountInUse))
        );
    }

    #[test]
    fn test_one_tx_two_out_atomic_fail() {
        let (genesis_block, mint_keypair) = GenesisBlock::new(1);
        let key1 = Keypair::new().pubkey();
        let key2 = Keypair::new().pubkey();
        let bank = Bank::new(&genesis_block);
        let spend = SystemInstruction::Move { tokens: 1 };
        let instructions = vec![
            Instruction {
                program_ids_index: 0,
                userdata: serialize(&spend).unwrap(),
                accounts: vec![0, 1],
            },
            Instruction {
                program_ids_index: 0,
                userdata: serialize(&spend).unwrap(),
                accounts: vec![0, 2],
            },
        ];

        let t1 = Transaction::new_with_instructions(
            &[&mint_keypair],
            &[key1, key2],
            genesis_block.last_id(),
            0,
            vec![system_program::id()],
            instructions,
        );
        let res = bank.process_transactions(&vec![t1.clone()]);
        assert_eq!(res.len(), 1);
        assert_eq!(bank.get_balance(&mint_keypair.pubkey()), 1);
        assert_eq!(bank.get_balance(&key1), 0);
        assert_eq!(bank.get_balance(&key2), 0);
        assert_eq!(
            bank.get_signature_status(&t1.signatures[0]),
            Some(Err(BankError::ProgramError(
                1,
                ProgramError::ResultWithNegativeTokens
            )))
        );
    }

    #[test]
    fn test_one_tx_two_out_atomic_pass() {
        let (genesis_block, mint_keypair) = GenesisBlock::new(2);
        let key1 = Keypair::new().pubkey();
        let key2 = Keypair::new().pubkey();
        let bank = Bank::new(&genesis_block);
        let t1 = SystemTransaction::new_move_many(
            &mint_keypair,
            &[(key1, 1), (key2, 1)],
            genesis_block.last_id(),
            0,
        );
        let res = bank.process_transactions(&vec![t1.clone()]);
        assert_eq!(res.len(), 1);
        assert_eq!(res[0], Ok(()));
        assert_eq!(bank.get_balance(&mint_keypair.pubkey()), 0);
        assert_eq!(bank.get_balance(&key1), 1);
        assert_eq!(bank.get_balance(&key2), 1);
        assert_eq!(bank.get_signature_status(&t1.signatures[0]), Some(Ok(())));
    }

    // This test demonstrates that fees are paid even when a program fails.
    #[test]
    fn test_detect_failed_duplicate_transactions_issue_1157() {
        let (genesis_block, mint_keypair) = GenesisBlock::new(2);
        let bank = Bank::new(&genesis_block);
        let dest = Keypair::new();

        // source with 0 program context
        let tx = SystemTransaction::new_account(
            &mint_keypair,
            dest.pubkey(),
            2,
            genesis_block.last_id(),
            1,
        );
        let signature = tx.signatures[0];
        assert!(!bank.has_signature(&signature));

        // Assert that process_transaction has filtered out Program Errors
        assert_eq!(bank.process_transaction(&tx), Ok(()));

        assert!(bank.has_signature(&signature));
        assert_eq!(
            bank.get_signature_status(&signature),
            Some(Err(BankError::ProgramError(
                0,
                ProgramError::ResultWithNegativeTokens
            )))
        );

        // The tokens didn't move, but the from address paid the transaction fee.
        assert_eq!(bank.get_balance(&dest.pubkey()), 0);

        // This should be the original balance minus the transaction fee.
        assert_eq!(bank.get_balance(&mint_keypair.pubkey()), 1);
    }

    #[test]
    fn test_account_not_found() {
        let (genesis_block, mint_keypair) = GenesisBlock::new(0);
        let bank = Bank::new(&genesis_block);
        let keypair = Keypair::new();
        assert_eq!(
            bank.transfer(1, &keypair, mint_keypair.pubkey(), genesis_block.last_id()),
            Err(BankError::AccountNotFound)
        );
        assert_eq!(bank.transaction_count(), 0);
    }

    #[test]
    fn test_insufficient_funds() {
        let (genesis_block, mint_keypair) = GenesisBlock::new(11_000);
        let bank = Bank::new(&genesis_block);
        let pubkey = Keypair::new().pubkey();
        bank.transfer(1_000, &mint_keypair, pubkey, genesis_block.last_id())
            .unwrap();
        assert_eq!(bank.transaction_count(), 1);
        assert_eq!(bank.get_balance(&pubkey), 1_000);
        let signature = bank
            .transfer(10_001, &mint_keypair, pubkey, genesis_block.last_id())
            .unwrap();
        assert_eq!(bank.transaction_count(), 1);
        assert!(bank.has_signature(&signature));
        assert_eq!(
            bank.get_signature_status(&signature),
            Some(Err(BankError::ProgramError(
                0,
                ProgramError::ResultWithNegativeTokens
            )))
        );

        let mint_pubkey = mint_keypair.pubkey();
        assert_eq!(bank.get_balance(&mint_pubkey), 10_000);
        assert_eq!(bank.get_balance(&pubkey), 1_000);
    }

    #[test]
    fn test_transfer_to_newb() {
        let (genesis_block, mint_keypair) = GenesisBlock::new(10_000);
        let bank = Bank::new(&genesis_block);
        let pubkey = Keypair::new().pubkey();
        bank.transfer(500, &mint_keypair, pubkey, genesis_block.last_id())
            .unwrap();
        assert_eq!(bank.get_balance(&pubkey), 500);
    }

    #[test]
    fn test_bank_deposit() {
        let (genesis_block, _mint_keypair) = GenesisBlock::new(100);
        let bank = Bank::new(&genesis_block);

        // Test new account
        let key = Keypair::new();
        bank.deposit(&key.pubkey(), 10);
        assert_eq!(bank.get_balance(&key.pubkey()), 10);

        // Existing account
        bank.deposit(&key.pubkey(), 3);
        assert_eq!(bank.get_balance(&key.pubkey()), 13);
    }

    #[test]
    fn test_bank_withdraw() {
        let (genesis_block, _mint_keypair) = GenesisBlock::new(100);
        let bank = Bank::new(&genesis_block);

        // Test no account
        let key = Keypair::new();
        assert_eq!(
            bank.withdraw(&key.pubkey(), 10),
            Err(BankError::AccountNotFound)
        );

        bank.deposit(&key.pubkey(), 3);
        assert_eq!(bank.get_balance(&key.pubkey()), 3);

        // Low balance
        assert_eq!(
            bank.withdraw(&key.pubkey(), 10),
            Err(BankError::InsufficientFundsForFee)
        );

        // Enough balance
        assert_eq!(bank.withdraw(&key.pubkey(), 2), Ok(()));
        assert_eq!(bank.get_balance(&key.pubkey()), 1);
    }

    #[test]
    fn test_bank_tx_fee() {
        let (genesis_block, mint_keypair) = GenesisBlock::new(100);
        let mut bank = Bank::new(&genesis_block);
        bank.leader = Pubkey::default();

        let key1 = Keypair::new();
        let key2 = Keypair::new();

        let tx = SystemTransaction::new_move(
            &mint_keypair,
            key1.pubkey(),
            2,
            genesis_block.last_id(),
            3,
        );
        let initial_balance = bank.get_balance(&bank.leader);
        assert_eq!(bank.process_transaction(&tx), Ok(()));
        assert_eq!(bank.get_balance(&bank.leader), initial_balance + 3);
        assert_eq!(bank.get_balance(&key1.pubkey()), 2);
        assert_eq!(bank.get_balance(&mint_keypair.pubkey()), 100 - 2 - 3);

        let tx = SystemTransaction::new_move(&key1, key2.pubkey(), 1, genesis_block.last_id(), 1);
        assert_eq!(bank.process_transaction(&tx), Ok(()));
        assert_eq!(bank.get_balance(&bank.leader), initial_balance + 4);
        assert_eq!(bank.get_balance(&key1.pubkey()), 0);
        assert_eq!(bank.get_balance(&key2.pubkey()), 1);
        assert_eq!(bank.get_balance(&mint_keypair.pubkey()), 100 - 2 - 3);
    }

    #[test]
    fn test_filter_program_errors_and_collect_fee() {
        let (genesis_block, mint_keypair) = GenesisBlock::new(100);
        let mut bank = Bank::new(&genesis_block);
        bank.leader = Pubkey::default();

        let key = Keypair::new();
        let tx1 =
            SystemTransaction::new_move(&mint_keypair, key.pubkey(), 2, genesis_block.last_id(), 3);
        let tx2 =
            SystemTransaction::new_move(&mint_keypair, key.pubkey(), 5, genesis_block.last_id(), 1);

        let results = vec![
            Ok(()),
            Err(BankError::ProgramError(
                1,
                ProgramError::ResultWithNegativeTokens,
            )),
        ];

        let initial_balance = bank.get_balance(&bank.leader);
        let results = bank.filter_program_errors_and_collect_fee(&vec![tx1, tx2], &results);
        assert_eq!(bank.get_balance(&bank.leader), initial_balance + 3 + 1);
        assert_eq!(results[0], Ok(()));
        assert_eq!(results[1], Ok(()));
    }

    #[test]
    fn test_debits_before_credits() {
        let (genesis_block, mint_keypair) = GenesisBlock::new(2);
        let bank = Bank::new(&genesis_block);
        let keypair = Keypair::new();
        let tx0 = SystemTransaction::new_account(
            &mint_keypair,
            keypair.pubkey(),
            2,
            genesis_block.last_id(),
            0,
        );
        let tx1 = SystemTransaction::new_account(
            &keypair,
            mint_keypair.pubkey(),
            1,
            genesis_block.last_id(),
            0,
        );
        let txs = vec![tx0, tx1];
        let results = bank.process_transactions(&txs);
        assert!(results[1].is_err());

        // Assert bad transactions aren't counted.
        assert_eq!(bank.transaction_count(), 1);
    }

    #[test]
    fn test_process_genesis() {
        let dummy_leader_id = Keypair::new().pubkey();
        let dummy_leader_tokens = 2;
        let (genesis_block, _) =
            GenesisBlock::new_with_leader(5, dummy_leader_id, dummy_leader_tokens);
        let bank = Bank::new(&genesis_block);
        assert_eq!(bank.get_balance(&genesis_block.mint_id), 3);
        assert_eq!(bank.get_balance(&dummy_leader_id), 1);
    }

    // Register n ticks and return the tick, slot and epoch indexes.
    fn register_ticks(bank: &Bank, n: u64) -> (u64, u64, u64) {
        for _ in 0..n {
            bank.register_tick(&Hash::default());
        }
        (bank.tick_index(), bank.slot_index(), bank.epoch_height())
    }

    #[test]
    fn test_tick_slot_epoch_indexes() {
        let (genesis_block, _) = GenesisBlock::new(5);
        let bank = Bank::new(&genesis_block);
        let ticks_per_slot = bank.ticks_per_slot();
        let slots_per_epoch = bank.slots_per_epoch();
        let ticks_per_epoch = ticks_per_slot * slots_per_epoch;

        // All indexes are zero-based.
        assert_eq!(register_ticks(&bank, 0), (0, 0, 0));

        // Slot index remains zero through the last tick.
        assert_eq!(
            register_ticks(&bank, ticks_per_slot - 1),
            (ticks_per_slot - 1, 0, 0)
        );

        // Cross a slot boundary.
        assert_eq!(register_ticks(&bank, 1), (0, 1, 0));

        // Cross an epoch boundary.
        assert_eq!(register_ticks(&bank, ticks_per_epoch), (0, 1, 1));
    }

    #[test]
    fn test_leader_schedule_bank() {
        let (genesis_block, _) = GenesisBlock::new(5);
        let bank = Bank::new(&genesis_block);
        assert!(bank.leader_schedule_bank().is_none());

        let bank = Bank::new_from_parent(&Arc::new(bank), &Pubkey::default());
        let ticks_per_offset = bank.leader_schedule_slot_offset * bank.ticks_per_slot();
        register_ticks(&bank, ticks_per_offset);
        assert_eq!(bank.slot_height(), bank.leader_schedule_slot_offset);

        let slot_height = bank.slots_per_epoch() - bank.leader_schedule_slot_offset;
        let bank = Bank::new_from_parent(&Arc::new(bank), &Pubkey::default());
        assert_eq!(
            bank.leader_schedule_bank().unwrap().slot_height(),
            slot_height
        );
    }

    #[test]
    fn test_interleaving_locks() {
        let (genesis_block, mint_keypair) = GenesisBlock::new(3);
        let bank = Bank::new(&genesis_block);
        let alice = Keypair::new();
        let bob = Keypair::new();

        let tx1 = SystemTransaction::new_account(
            &mint_keypair,
            alice.pubkey(),
            1,
            genesis_block.last_id(),
            0,
        );
        let pay_alice = vec![tx1];

        let lock_result = bank.lock_accounts(&pay_alice);
        let results_alice =
            bank.load_execute_and_commit_transactions(&pay_alice, lock_result, MAX_ENTRY_IDS);
        assert_eq!(results_alice[0], Ok(()));

        // try executing an interleaved transfer twice
        assert_eq!(
            bank.transfer(1, &mint_keypair, bob.pubkey(), genesis_block.last_id()),
            Err(BankError::AccountInUse)
        );
        // the second time should fail as well
        // this verifies that `unlock_accounts` doesn't unlock `AccountInUse` accounts
        assert_eq!(
            bank.transfer(1, &mint_keypair, bob.pubkey(), genesis_block.last_id()),
            Err(BankError::AccountInUse)
        );

        bank.unlock_accounts(&pay_alice, &results_alice);

        assert!(bank
            .transfer(2, &mint_keypair, bob.pubkey(), genesis_block.last_id())
            .is_ok());
    }

    #[test]
    fn test_program_ids() {
        let system = Pubkey::new(&[
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0,
        ]);
        let native = Pubkey::new(&[
            1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0,
        ]);
        let bpf = Pubkey::new(&[
            128, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0,
        ]);
        let budget = Pubkey::new(&[
            129, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0,
        ]);
        let storage = Pubkey::new(&[
            130, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0,
        ]);
        let token = Pubkey::new(&[
            131, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0,
        ]);
        let vote = Pubkey::new(&[
            132, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0,
        ]);
        let storage_system = Pubkey::new(&[
            133, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0,
        ]);

        assert_eq!(system_program::id(), system);
        assert_eq!(native_loader::id(), native);
        assert_eq!(bpf_loader::id(), bpf);
        assert_eq!(budget_program::id(), budget);
        assert_eq!(storage_program::id(), storage);
        assert_eq!(token_program::id(), token);
        assert_eq!(vote_program::id(), vote);
        assert_eq!(storage_program::system_id(), storage_system);
    }

    #[test]
    fn test_program_id_uniqueness() {
        let mut unique = HashSet::new();
        let ids = vec![
            system_program::id(),
            native_loader::id(),
            bpf_loader::id(),
            budget_program::id(),
            storage_program::id(),
            token_program::id(),
            vote_program::id(),
            storage_program::system_id(),
        ];
        assert!(ids.into_iter().all(move |id| unique.insert(id)));
    }

    #[test]
    fn test_bank_pay_to_self() {
        let (genesis_block, mint_keypair) = GenesisBlock::new(1);
        let key1 = Keypair::new();
        let bank = Bank::new(&genesis_block);

        bank.transfer(1, &mint_keypair, key1.pubkey(), genesis_block.last_id())
            .unwrap();
        assert_eq!(bank.get_balance(&key1.pubkey()), 1);
        let tx = SystemTransaction::new_move(&key1, key1.pubkey(), 1, genesis_block.last_id(), 0);
        let res = bank.process_transactions(&vec![tx.clone()]);
        assert_eq!(res.len(), 1);
        assert_eq!(bank.get_balance(&key1.pubkey()), 1);
        res[0].clone().unwrap_err();
    }

    /// Verify that the parent's vector is computed correctly
    #[test]
    fn test_bank_parents() {
        let (genesis_block, _) = GenesisBlock::new(1);
        let parent = Arc::new(Bank::new(&genesis_block));

        let bank = Bank::new_from_parent(&parent, &Pubkey::default());
        assert!(Arc::ptr_eq(&bank.parents()[0], &parent));
    }

    /// Verifies that last ids and status cache are correctly referenced from parent
    #[test]
    fn test_bank_parent_duplicate_signature() {
        let (genesis_block, mint_keypair) = GenesisBlock::new(2);
        let key1 = Keypair::new();
        let parent = Arc::new(Bank::new(&genesis_block));

        let tx = SystemTransaction::new_move(
            &mint_keypair,
            key1.pubkey(),
            1,
            genesis_block.last_id(),
            0,
        );
        assert_eq!(parent.process_transaction(&tx), Ok(()));
        let bank = Bank::new_from_parent(&parent, &Pubkey::default());
        assert_eq!(
            bank.process_transaction(&tx),
            Err(BankError::DuplicateSignature)
        );
    }

    /// Verifies that last ids and accounts are correctly referenced from parent
    #[test]
    fn test_bank_parent_account_spend() {
        let (genesis_block, mint_keypair) = GenesisBlock::new(2);
        let key1 = Keypair::new();
        let key2 = Keypair::new();
        let parent = Arc::new(Bank::new(&genesis_block));

        let tx = SystemTransaction::new_move(
            &mint_keypair,
            key1.pubkey(),
            1,
            genesis_block.last_id(),
            0,
        );
        assert_eq!(parent.process_transaction(&tx), Ok(()));
        let bank = Bank::new_from_parent(&parent, &Pubkey::default());
        let tx = SystemTransaction::new_move(&key1, key2.pubkey(), 1, genesis_block.last_id(), 0);
        assert_eq!(bank.process_transaction(&tx), Ok(()));
        assert_eq!(parent.get_signature_status(&tx.signatures[0]), None);
    }

    #[test]
    fn test_hash_internal_state() {
        let (genesis_block, mint_keypair) = GenesisBlock::new(2_000);
        let bank0 = Bank::new(&genesis_block);
        let bank1 = Bank::new(&genesis_block);
        let initial_state = bank0.hash_internal_state();
        assert_eq!(bank1.hash_internal_state(), initial_state);

        let pubkey = Keypair::new().pubkey();
        bank0
            .transfer(1_000, &mint_keypair, pubkey, bank0.last_id())
            .unwrap();
        assert_ne!(bank0.hash_internal_state(), initial_state);
        bank1
            .transfer(1_000, &mint_keypair, pubkey, bank1.last_id())
            .unwrap();
        assert_eq!(bank0.hash_internal_state(), bank1.hash_internal_state());

        // Checkpointing should not change its state
        let bank2 = Bank::new_from_parent(&Arc::new(bank1), &Pubkey::default());
        assert_eq!(bank0.hash_internal_state(), bank2.hash_internal_state());
    }

    #[test]
    fn test_hash_internal_state_parents() {
        let bank0 = Bank::new(&GenesisBlock::new(10).0);
        let bank1 = Bank::new(&GenesisBlock::new(20).0);
        assert_ne!(bank0.hash_internal_state(), bank1.hash_internal_state());
    }

    /// Verifies that last ids and accounts are correctly referenced from parent
    #[test]
    fn test_bank_merge_parents() {
        let (genesis_block, mint_keypair) = GenesisBlock::new(2);
        let key1 = Keypair::new();
        let key2 = Keypair::new();
        let parent = Arc::new(Bank::new(&genesis_block));

        let tx_move_mint_to_1 = SystemTransaction::new_move(
            &mint_keypair,
            key1.pubkey(),
            1,
            genesis_block.last_id(),
            0,
        );
        assert_eq!(parent.process_transaction(&tx_move_mint_to_1), Ok(()));
        let mut bank = Bank::new_from_parent(&parent, &Pubkey::default());
        let tx_move_1_to_2 =
            SystemTransaction::new_move(&key1, key2.pubkey(), 1, genesis_block.last_id(), 0);
        assert_eq!(bank.process_transaction(&tx_move_1_to_2), Ok(()));
        assert_eq!(
            parent.get_signature_status(&tx_move_1_to_2.signatures[0]),
            None
        );

        for _ in 0..3 {
            // first time these should match what happened above, assert that parents are ok
            assert_eq!(bank.get_balance(&key1.pubkey()), 0);
            assert_eq!(bank.get_balance(&key2.pubkey()), 1);
            assert_eq!(
                bank.get_signature_status(&tx_move_mint_to_1.signatures[0]),
                Some(Ok(()))
            );
            assert_eq!(
                bank.get_signature_status(&tx_move_1_to_2.signatures[0]),
                Some(Ok(()))
            );

            // works iteration 0, no-ops on iteration 1 and 2
            bank.merge_parents();
        }
    }

}
