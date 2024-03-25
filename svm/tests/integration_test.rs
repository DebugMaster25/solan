#![cfg(test)]

use {
    crate::mock_bank::MockBankCallback,
    solana_bpf_loader_program::syscalls::{
        SyscallAbort, SyscallGetClockSysvar, SyscallInvokeSignedRust, SyscallLog, SyscallMemcpy,
        SyscallMemset, SyscallSetReturnData,
    },
    solana_program_runtime::{
        compute_budget::ComputeBudget,
        invoke_context::InvokeContext,
        loaded_programs::{
            BlockRelation, ForkGraph, LoadedProgram, ProgramCache, ProgramRuntimeEnvironments,
        },
        runtime_config::RuntimeConfig,
        solana_rbpf::{
            program::{BuiltinFunction, BuiltinProgram, FunctionRegistry},
            vm::Config,
        },
        timings::ExecuteTimings,
    },
    solana_sdk::{
        account::{AccountSharedData, ReadableAccount, WritableAccount},
        bpf_loader,
        clock::{Clock, Epoch, Slot, UnixTimestamp},
        epoch_schedule::EpochSchedule,
        fee::FeeStructure,
        hash::Hash,
        instruction::CompiledInstruction,
        message::{Message, MessageHeader},
        native_loader,
        pubkey::Pubkey,
        signature::Signature,
        sysvar::SysvarId,
        transaction::{SanitizedTransaction, Transaction, TransactionError},
    },
    solana_svm::{
        account_loader::TransactionCheckResult,
        transaction_error_metrics::TransactionErrorMetrics,
        transaction_processor::{
            ExecutionRecordingConfig, TransactionBatchProcessor, TransactionProcessingCallback,
        },
        transaction_results::TransactionExecutionResult,
    },
    std::{
        cmp::Ordering,
        env,
        fs::{self, File},
        io::Read,
        sync::{Arc, RwLock},
        time::{SystemTime, UNIX_EPOCH},
    },
};

// This module contains the implementation of TransactionProcessingCallback
mod mock_bank;

const BPF_LOADER_NAME: &str = "solana_bpf_loader_program";
const SYSTEM_PROGRAM_NAME: &str = "system_program";
const DEPLOYMENT_SLOT: u64 = 0;
const EXECUTION_SLOT: u64 = 5; // The execution slot must be greater than the deployment slot
const DEPLOYMENT_EPOCH: u64 = 0;
const EXECUTION_EPOCH: u64 = 2; // The execution epoch must be greater than the deployment epoch

struct MockForkGraph {}

impl ForkGraph for MockForkGraph {
    fn relationship(&self, a: Slot, b: Slot) -> BlockRelation {
        match a.cmp(&b) {
            Ordering::Less => BlockRelation::Ancestor,
            Ordering::Equal => BlockRelation::Equal,
            Ordering::Greater => BlockRelation::Descendant,
        }
    }

    fn slot_epoch(&self, _slot: Slot) -> Option<Epoch> {
        Some(0)
    }
}

fn create_custom_environment<'a>() -> BuiltinProgram<InvokeContext<'a>> {
    let compute_budget = ComputeBudget::default();
    let vm_config = Config {
        max_call_depth: compute_budget.max_call_depth,
        stack_frame_size: compute_budget.stack_frame_size,
        enable_address_translation: true,
        enable_stack_frame_gaps: true,
        instruction_meter_checkpoint_distance: 10000,
        enable_instruction_meter: true,
        enable_instruction_tracing: true,
        enable_symbol_and_section_labels: true,
        reject_broken_elfs: true,
        noop_instruction_rate: 256,
        sanitize_user_provided_values: true,
        external_internal_function_hash_collision: false,
        reject_callx_r10: false,
        enable_sbpf_v1: true,
        enable_sbpf_v2: false,
        optimize_rodata: false,
        new_elf_parser: false,
        aligned_memory_mapping: true,
    };

    // These functions are system calls the compile contract calls during execution, so they
    // need to be registered.
    let mut function_registry = FunctionRegistry::<BuiltinFunction<InvokeContext>>::default();
    function_registry
        .register_function_hashed(*b"abort", SyscallAbort::vm)
        .expect("Registration failed");
    function_registry
        .register_function_hashed(*b"sol_log_", SyscallLog::vm)
        .expect("Registration failed");
    function_registry
        .register_function_hashed(*b"sol_memcpy_", SyscallMemcpy::vm)
        .expect("Registration failed");
    function_registry
        .register_function_hashed(*b"sol_memset_", SyscallMemset::vm)
        .expect("Registration failed");

    function_registry
        .register_function_hashed(*b"sol_invoke_signed_rust", SyscallInvokeSignedRust::vm)
        .expect("Registration failed");

    function_registry
        .register_function_hashed(*b"sol_set_return_data", SyscallSetReturnData::vm)
        .expect("Registration failed");

    function_registry
        .register_function_hashed(*b"sol_get_clock_sysvar", SyscallGetClockSysvar::vm)
        .expect("Registration failed");

    BuiltinProgram::new_loader(vm_config, function_registry)
}

fn create_executable_environment(
    mock_bank: &mut MockBankCallback,
) -> (ProgramCache<MockForkGraph>, Vec<Pubkey>) {
    let mut program_cache = ProgramCache::<MockForkGraph>::new(0, 20);

    // We must register the bpf loader account as a loadable account, otherwise programs
    // won't execute.
    let account_data = native_loader::create_loadable_account_with_fields(
        BPF_LOADER_NAME,
        (5000, DEPLOYMENT_EPOCH),
    );
    mock_bank
        .account_shared_data
        .insert(bpf_loader::id(), account_data);

    // The bpf loader needs an executable as well
    program_cache.assign_program(
        bpf_loader::id(),
        Arc::new(LoadedProgram::new_builtin(
            DEPLOYMENT_SLOT,
            BPF_LOADER_NAME.len(),
            solana_bpf_loader_program::Entrypoint::vm,
        )),
    );

    // In order to perform a transference of native tokens using the system instruction,
    // the system program builtin must be registered.
    let account_data = native_loader::create_loadable_account_with_fields(
        SYSTEM_PROGRAM_NAME,
        (5000, DEPLOYMENT_EPOCH),
    );
    mock_bank
        .account_shared_data
        .insert(solana_system_program::id(), account_data);
    program_cache.assign_program(
        solana_system_program::id(),
        Arc::new(LoadedProgram::new_builtin(
            DEPLOYMENT_SLOT,
            SYSTEM_PROGRAM_NAME.len(),
            solana_system_program::system_processor::Entrypoint::vm,
        )),
    );

    program_cache.environments = ProgramRuntimeEnvironments {
        program_runtime_v1: Arc::new(create_custom_environment()),
        // We are not using program runtime v2
        program_runtime_v2: Arc::new(BuiltinProgram::new_loader(
            Config::default(),
            FunctionRegistry::default(),
        )),
    };

    program_cache.fork_graph = Some(Arc::new(RwLock::new(MockForkGraph {})));

    // We must fill in the sysvar cache entries
    let time_now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs() as i64;
    let clock = Clock {
        slot: DEPLOYMENT_SLOT,
        epoch_start_timestamp: time_now.saturating_sub(10) as UnixTimestamp,
        epoch: DEPLOYMENT_EPOCH,
        leader_schedule_epoch: DEPLOYMENT_EPOCH,
        unix_timestamp: time_now as UnixTimestamp,
    };

    let mut account_data = AccountSharedData::default();
    account_data.set_data(bincode::serialize(&clock).unwrap());
    mock_bank
        .account_shared_data
        .insert(Clock::id(), account_data);

    // Inform SVM of the registered builins
    let registered_built_ins = vec![bpf_loader::id(), solana_system_program::id()];
    (program_cache, registered_built_ins)
}

fn load_program(name: String) -> Vec<u8> {
    // Loading the program file
    let mut dir = env::current_dir().unwrap();
    dir.push("tests");
    dir.push("example-programs");
    dir.push(name.as_str());
    let name = name.replace('-', "_");
    dir.push(name + "_program.so");
    let mut file = File::open(dir.clone()).expect("file not found");
    let metadata = fs::metadata(dir).expect("Unable to read metadata");
    let mut buffer = vec![0; metadata.len() as usize];
    file.read_exact(&mut buffer).expect("Buffer overflow");
    buffer
}

fn prepare_transactions(
    mock_bank: &mut MockBankCallback,
) -> (Vec<SanitizedTransaction>, Vec<TransactionCheckResult>) {
    let mut all_transactions = Vec::new();
    let mut transaction_checks = Vec::new();

    // A transaction that works without any account
    let key1 = Pubkey::new_unique();
    let fee_payer = Pubkey::new_unique();
    let message = Message {
        account_keys: vec![fee_payer, key1],
        header: MessageHeader {
            num_required_signatures: 1,
            num_readonly_signed_accounts: 0,
            num_readonly_unsigned_accounts: 0,
        },
        instructions: vec![CompiledInstruction {
            program_id_index: 1,
            accounts: vec![],
            data: vec![],
        }],
        recent_blockhash: Hash::default(),
    };

    let transaction = Transaction {
        signatures: vec![Signature::new_unique()],
        message,
    };
    let sanitized_transaction =
        SanitizedTransaction::try_from_legacy_transaction(transaction).unwrap();
    all_transactions.push(sanitized_transaction);
    transaction_checks.push((Ok(()), None, Some(20)));

    // Loading the program file
    let buffer = load_program("hello-solana".to_string());

    // The program account must have funds and hold the executable binary
    let mut account_data = AccountSharedData::default();
    // The executable account owner must be one of the loaders.
    account_data.set_owner(bpf_loader::id());
    account_data.set_data(buffer);
    account_data.set_executable(true);
    account_data.set_lamports(25);
    mock_bank.account_shared_data.insert(key1, account_data);

    // The transaction fee payer must have enough funds
    let mut account_data = AccountSharedData::default();
    account_data.set_lamports(80000);
    mock_bank
        .account_shared_data
        .insert(fee_payer, account_data);

    // A simple funds transfer between accounts
    let transfer_program_account = Pubkey::new_unique();
    let sender = Pubkey::new_unique();
    let recipient = Pubkey::new_unique();
    let fee_payer = Pubkey::new_unique();
    let system_account = Pubkey::from([0u8; 32]);
    let message = Message {
        account_keys: vec![
            fee_payer,
            sender,
            transfer_program_account,
            recipient,
            system_account,
        ],
        header: MessageHeader {
            // The signers must appear in the `account_keys` vector in positions whose index is
            // less than `num_required_signatures`
            num_required_signatures: 2,
            num_readonly_signed_accounts: 0,
            num_readonly_unsigned_accounts: 0,
        },
        instructions: vec![CompiledInstruction {
            program_id_index: 2,
            accounts: vec![1, 3, 4],
            data: vec![0, 0, 0, 0, 0, 0, 0, 10],
        }],
        recent_blockhash: Hash::default(),
    };

    let transaction = Transaction {
        signatures: vec![Signature::new_unique(), Signature::new_unique()],
        message,
    };

    let sanitized_transaction =
        SanitizedTransaction::try_from_legacy_transaction(transaction).unwrap();
    all_transactions.push(sanitized_transaction);
    transaction_checks.push((Ok(()), None, Some(20)));

    // Setting up the accounts for the transfer

    // fee payer
    let mut account_data = AccountSharedData::default();
    account_data.set_lamports(80000);
    mock_bank
        .account_shared_data
        .insert(fee_payer, account_data);

    let buffer = load_program("simple-transfer".to_string());

    // The program account must have funds and hold the executable binary
    let mut account_data = AccountSharedData::default();
    // The executable account owner must be one of the loaders.
    account_data.set_owner(bpf_loader::id());
    account_data.set_data(buffer);
    account_data.set_executable(true);
    account_data.set_lamports(25);
    mock_bank
        .account_shared_data
        .insert(transfer_program_account, account_data);

    // sender
    let mut account_data = AccountSharedData::default();
    account_data.set_lamports(900000);
    mock_bank.account_shared_data.insert(sender, account_data);

    // recipient
    let mut account_data = AccountSharedData::default();
    account_data.set_lamports(900000);
    mock_bank
        .account_shared_data
        .insert(recipient, account_data);

    // The program account is set in `create_executable_environment`

    // A program that utilizes a Sysvar
    let program_account = Pubkey::new_unique();
    let fee_payer = Pubkey::new_unique();
    let message = Message {
        account_keys: vec![fee_payer, program_account],
        header: MessageHeader {
            num_required_signatures: 1,
            num_readonly_signed_accounts: 0,
            num_readonly_unsigned_accounts: 0,
        },
        instructions: vec![CompiledInstruction {
            program_id_index: 1,
            accounts: vec![],
            data: vec![],
        }],
        recent_blockhash: Hash::default(),
    };

    let transaction = Transaction {
        signatures: vec![Signature::new_unique()],
        message,
    };
    let sanitized_transaction =
        SanitizedTransaction::try_from_legacy_transaction(transaction).unwrap();
    all_transactions.push(sanitized_transaction);
    transaction_checks.push((Ok(()), None, Some(20)));

    let mut account_data = AccountSharedData::default();
    account_data.set_lamports(80000);
    mock_bank
        .account_shared_data
        .insert(fee_payer, account_data);

    let buffer = load_program("clock-sysvar".to_string());

    // The program account must have funds and hold the executable binary
    let mut account_data = AccountSharedData::default();
    // The executable account owner must be one of the loaders.
    account_data.set_owner(bpf_loader::id());
    account_data.set_data(buffer);
    account_data.set_executable(true);
    account_data.set_lamports(25);
    mock_bank
        .account_shared_data
        .insert(program_account, account_data);

    // A transaction that fails
    let sender = Pubkey::new_unique();
    let recipient = Pubkey::new_unique();
    let fee_payer = Pubkey::new_unique();
    let system_account = Pubkey::new_from_array([0; 32]);
    let mut data = 900050u64.to_be_bytes().to_vec();
    while data.len() < 8 {
        data.insert(0, 0);
    }

    let message = Message {
        account_keys: vec![
            fee_payer,
            sender,
            transfer_program_account,
            recipient,
            system_account,
        ],
        header: MessageHeader {
            num_required_signatures: 2,
            num_readonly_signed_accounts: 0,
            num_readonly_unsigned_accounts: 0,
        },
        instructions: vec![CompiledInstruction {
            program_id_index: 2,
            accounts: vec![1, 3, 4],
            data,
        }],
        recent_blockhash: Hash::default(),
    };
    let transaction = Transaction {
        signatures: vec![Signature::new_unique(), Signature::new_unique()],
        message,
    };
    let sanitized_transaction =
        SanitizedTransaction::try_from_legacy_transaction(transaction).unwrap();
    all_transactions.push(sanitized_transaction.clone());
    transaction_checks.push((Ok(()), None, Some(20)));

    // fee payer
    let mut account_data = AccountSharedData::default();
    account_data.set_lamports(80000);
    mock_bank
        .account_shared_data
        .insert(fee_payer, account_data);

    // Sender without enough funds
    let mut account_data = AccountSharedData::default();
    account_data.set_lamports(900000);
    mock_bank.account_shared_data.insert(sender, account_data);

    // recipient
    let mut account_data = AccountSharedData::default();
    account_data.set_lamports(900000);
    mock_bank
        .account_shared_data
        .insert(recipient, account_data);

    // A transaction whose verification has already failed
    all_transactions.push(sanitized_transaction);
    transaction_checks.push((Err(TransactionError::BlockhashNotFound), None, Some(20)));

    (all_transactions, transaction_checks)
}

#[test]
fn svm_integration() {
    let mut mock_bank = MockBankCallback::default();
    let (transactions, mut check_results) = prepare_transactions(&mut mock_bank);
    let (program_cache, builtins) = create_executable_environment(&mut mock_bank);
    let program_cache = Arc::new(RwLock::new(program_cache));
    let batch_processor = TransactionBatchProcessor::<MockForkGraph>::new(
        EXECUTION_SLOT,
        EXECUTION_EPOCH,
        EpochSchedule::default(),
        FeeStructure::default(),
        Arc::new(RuntimeConfig::default()),
        program_cache.clone(),
    );

    // The sysvars must be put in the cache
    batch_processor
        .sysvar_cache
        .write()
        .unwrap()
        .fill_missing_entries(|pubkey, callback| {
            if let Some(account) = mock_bank.get_account_shared_data(pubkey) {
                callback(account.data());
            }
        });

    let mut error_counter = TransactionErrorMetrics::default();
    let recording_config = ExecutionRecordingConfig {
        enable_log_recording: true,
        enable_return_data_recording: true,
        enable_cpi_recording: false,
    };
    let mut timings = ExecuteTimings::default();

    let result = batch_processor.load_and_execute_sanitized_transactions(
        &mock_bank,
        &transactions,
        check_results.as_mut_slice(),
        &mut error_counter,
        recording_config,
        &mut timings,
        None,
        builtins.iter(),
        None,
        false,
    );

    assert_eq!(result.execution_results.len(), 5);
    assert!(result.execution_results[0]
        .details()
        .unwrap()
        .status
        .is_ok());
    let logs = result.execution_results[0]
        .details()
        .unwrap()
        .log_messages
        .as_ref()
        .unwrap();
    assert!(logs.contains(&"Program log: Hello, Solana!".to_string()));

    assert!(result.execution_results[1]
        .details()
        .unwrap()
        .status
        .is_ok());

    // The SVM does not commit the account changes in MockBank
    let recipient_key = transactions[1].message().account_keys()[3];
    let recipient_data = result.loaded_transactions[1]
        .0
        .as_ref()
        .unwrap()
        .accounts
        .iter()
        .find(|key| key.0 == recipient_key)
        .unwrap();
    assert_eq!(recipient_data.1.lamports(), 900010);

    let return_data = result.execution_results[2]
        .details()
        .unwrap()
        .return_data
        .as_ref()
        .unwrap();
    let time = i64::from_be_bytes(return_data.data[0..8].try_into().unwrap());
    let clock_data = mock_bank.get_account_shared_data(&Clock::id()).unwrap();
    let clock_info: Clock = bincode::deserialize(clock_data.data()).unwrap();
    assert_eq!(clock_info.unix_timestamp, time);

    assert!(result.execution_results[3]
        .details()
        .unwrap()
        .status
        .is_err());
    assert!(result.execution_results[3]
        .details()
        .unwrap()
        .log_messages
        .as_ref()
        .unwrap()
        .contains(&"Transfer: insufficient lamports 900000, need 900050".to_string()));

    assert!(matches!(
        result.execution_results[4],
        TransactionExecutionResult::NotExecuted(TransactionError::BlockhashNotFound)
    ));
}
