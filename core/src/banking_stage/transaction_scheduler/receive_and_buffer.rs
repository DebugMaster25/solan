use {
    super::{
        scheduler_metrics::{SchedulerCountMetrics, SchedulerTimingMetrics},
        transaction_priority_id::TransactionPriorityId,
        transaction_state::TransactionState,
        transaction_state_container::{
            SharedBytes, StateContainer, TransactionViewState, TransactionViewStateContainer,
            EXTRA_CAPACITY,
        },
    },
    crate::banking_stage::{
        consumer::Consumer, decision_maker::BufferedPacketsDecision,
        immutable_deserialized_packet::ImmutableDeserializedPacket,
        packet_deserializer::PacketDeserializer, packet_filter::MAX_ALLOWED_PRECOMPILE_SIGNATURES,
        scheduler_messages::MaxAge,
        transaction_scheduler::transaction_state::SanitizedTransactionTTL,
        TransactionStateContainer,
    },
    agave_banking_stage_ingress_types::{BankingPacketBatch, BankingPacketReceiver},
    agave_transaction_view::{
        resolved_transaction_view::ResolvedTransactionView,
        transaction_version::TransactionVersion, transaction_view::SanitizedTransactionView,
    },
    arrayvec::ArrayVec,
    core::time::Duration,
    crossbeam_channel::{RecvTimeoutError, TryRecvError},
    solana_accounts_db::account_locks::validate_account_locks,
    solana_cost_model::cost_model::CostModel,
    solana_measure::measure_us,
    solana_runtime::{bank::Bank, bank_forks::BankForks},
    solana_runtime_transaction::{
        runtime_transaction::RuntimeTransaction, transaction_meta::StaticMeta,
        transaction_with_meta::TransactionWithMeta,
    },
    solana_sdk::{
        address_lookup_table::state::estimate_last_valid_slot,
        clock::{Epoch, Slot, MAX_PROCESSING_AGE},
        fee::FeeBudgetLimits,
        saturating_add_assign,
        transaction::{MessageHash, SanitizedTransaction},
    },
    solana_svm::transaction_error_metrics::TransactionErrorMetrics,
    solana_svm_transaction::svm_message::SVMMessage,
    std::{
        sync::{Arc, RwLock},
        time::Instant,
    },
};

pub(crate) trait ReceiveAndBuffer {
    type Transaction: TransactionWithMeta + Send + Sync;
    type Container: StateContainer<Self::Transaction> + Send + Sync;

    /// Return Err if the receiver is disconnected AND no packets were
    /// received. Otherwise return Ok(num_received).
    fn receive_and_buffer_packets(
        &mut self,
        container: &mut Self::Container,
        timing_metrics: &mut SchedulerTimingMetrics,
        count_metrics: &mut SchedulerCountMetrics,
        decision: &BufferedPacketsDecision,
    ) -> Result<usize, ()>;
}

pub(crate) struct SanitizedTransactionReceiveAndBuffer {
    /// Packet/Transaction ingress.
    packet_receiver: PacketDeserializer,
    bank_forks: Arc<RwLock<BankForks>>,

    forwarding_enabled: bool,
}

impl ReceiveAndBuffer for SanitizedTransactionReceiveAndBuffer {
    type Transaction = RuntimeTransaction<SanitizedTransaction>;
    type Container = TransactionStateContainer<Self::Transaction>;

    /// Returns whether the packet receiver is still connected.
    fn receive_and_buffer_packets(
        &mut self,
        container: &mut Self::Container,
        timing_metrics: &mut SchedulerTimingMetrics,
        count_metrics: &mut SchedulerCountMetrics,
        decision: &BufferedPacketsDecision,
    ) -> Result<usize, ()> {
        const MAX_RECEIVE_PACKETS: usize = 5_000;
        const MAX_PACKET_RECEIVE_TIME: Duration = Duration::from_millis(10);
        let (recv_timeout, should_buffer) = match decision {
            BufferedPacketsDecision::Consume(_) | BufferedPacketsDecision::Hold => (
                if container.is_empty() {
                    MAX_PACKET_RECEIVE_TIME
                } else {
                    Duration::ZERO
                },
                true,
            ),
            BufferedPacketsDecision::Forward => (MAX_PACKET_RECEIVE_TIME, self.forwarding_enabled),
            BufferedPacketsDecision::ForwardAndHold => (MAX_PACKET_RECEIVE_TIME, true),
        };

        let (received_packet_results, receive_time_us) = measure_us!(self
            .packet_receiver
            .receive_packets(recv_timeout, MAX_RECEIVE_PACKETS, |packet| {
                packet.check_excessive_precompiles()?;
                Ok(packet)
            }));

        timing_metrics.update(|timing_metrics| {
            saturating_add_assign!(timing_metrics.receive_time_us, receive_time_us);
        });

        let num_received = match received_packet_results {
            Ok(receive_packet_results) => {
                let num_received_packets = receive_packet_results.deserialized_packets.len();

                count_metrics.update(|count_metrics| {
                    saturating_add_assign!(count_metrics.num_received, num_received_packets);
                });

                if should_buffer {
                    let (_, buffer_time_us) = measure_us!(self.buffer_packets(
                        container,
                        timing_metrics,
                        count_metrics,
                        receive_packet_results.deserialized_packets
                    ));
                    timing_metrics.update(|timing_metrics| {
                        saturating_add_assign!(timing_metrics.buffer_time_us, buffer_time_us);
                    });
                } else {
                    count_metrics.update(|count_metrics| {
                        saturating_add_assign!(
                            count_metrics.num_dropped_on_receive,
                            num_received_packets
                        );
                    });
                }
                num_received_packets
            }
            Err(RecvTimeoutError::Timeout) => 0,
            Err(RecvTimeoutError::Disconnected) => return Err(()),
        };

        Ok(num_received)
    }
}

impl SanitizedTransactionReceiveAndBuffer {
    pub fn new(
        packet_receiver: PacketDeserializer,
        bank_forks: Arc<RwLock<BankForks>>,
        forwarding_enabled: bool,
    ) -> Self {
        Self {
            packet_receiver,
            bank_forks,
            forwarding_enabled,
        }
    }

    fn buffer_packets(
        &mut self,
        container: &mut TransactionStateContainer<RuntimeTransaction<SanitizedTransaction>>,
        _timing_metrics: &mut SchedulerTimingMetrics,
        count_metrics: &mut SchedulerCountMetrics,
        packets: Vec<ImmutableDeserializedPacket>,
    ) {
        // Convert to Arcs
        let packets: Vec<_> = packets.into_iter().map(Arc::new).collect();
        // Sanitize packets, generate IDs, and insert into the container.
        let (root_bank, working_bank) = {
            let bank_forks = self.bank_forks.read().unwrap();
            let root_bank = bank_forks.root_bank();
            let working_bank = bank_forks.working_bank();
            (root_bank, working_bank)
        };
        let alt_resolved_slot = root_bank.slot();
        let sanitized_epoch = root_bank.epoch();
        let transaction_account_lock_limit = working_bank.get_transaction_account_lock_limit();
        let vote_only = working_bank.vote_only_bank();

        const CHUNK_SIZE: usize = 128;
        let lock_results: [_; CHUNK_SIZE] = core::array::from_fn(|_| Ok(()));

        let mut arc_packets = ArrayVec::<_, CHUNK_SIZE>::new();
        let mut transactions = ArrayVec::<_, CHUNK_SIZE>::new();
        let mut max_ages = ArrayVec::<_, CHUNK_SIZE>::new();
        let mut fee_budget_limits_vec = ArrayVec::<_, CHUNK_SIZE>::new();

        let mut error_counts = TransactionErrorMetrics::default();
        for chunk in packets.chunks(CHUNK_SIZE) {
            let mut post_sanitization_count: usize = 0;
            chunk
                .iter()
                .filter_map(|packet| {
                    packet
                        .build_sanitized_transaction(
                            vote_only,
                            root_bank.as_ref(),
                            root_bank.get_reserved_account_keys(),
                        )
                        .map(|(tx, deactivation_slot)| (packet.clone(), tx, deactivation_slot))
                })
                .inspect(|_| saturating_add_assign!(post_sanitization_count, 1))
                .filter(|(_packet, tx, _deactivation_slot)| {
                    validate_account_locks(
                        tx.message().account_keys(),
                        transaction_account_lock_limit,
                    )
                    .is_ok()
                })
                .filter_map(|(packet, tx, deactivation_slot)| {
                    tx.compute_budget_instruction_details()
                        .sanitize_and_convert_to_compute_budget_limits(&working_bank.feature_set)
                        .map(|compute_budget| {
                            (packet, tx, deactivation_slot, compute_budget.into())
                        })
                        .ok()
                })
                .for_each(|(packet, tx, deactivation_slot, fee_budget_limits)| {
                    arc_packets.push(packet);
                    transactions.push(tx);
                    max_ages.push(calculate_max_age(
                        sanitized_epoch,
                        deactivation_slot,
                        alt_resolved_slot,
                    ));
                    fee_budget_limits_vec.push(fee_budget_limits);
                });

            let check_results = working_bank.check_transactions(
                &transactions,
                &lock_results[..transactions.len()],
                MAX_PROCESSING_AGE,
                &mut error_counts,
            );
            let post_lock_validation_count = transactions.len();

            let mut post_transaction_check_count: usize = 0;
            let mut num_dropped_on_capacity: usize = 0;
            let mut num_buffered: usize = 0;
            for ((((packet, transaction), max_age), fee_budget_limits), _check_result) in
                arc_packets
                    .drain(..)
                    .zip(transactions.drain(..))
                    .zip(max_ages.drain(..))
                    .zip(fee_budget_limits_vec.drain(..))
                    .zip(check_results)
                    .filter(|(_, check_result)| check_result.is_ok())
                    .filter(|((((_, tx), _), _), _)| {
                        Consumer::check_fee_payer_unlocked(&working_bank, tx, &mut error_counts)
                            .is_ok()
                    })
            {
                saturating_add_assign!(post_transaction_check_count, 1);

                let (priority, cost) =
                    calculate_priority_and_cost(&transaction, &fee_budget_limits, &working_bank);
                let transaction_ttl = SanitizedTransactionTTL {
                    transaction,
                    max_age,
                };

                if container.insert_new_transaction(transaction_ttl, packet, priority, cost) {
                    saturating_add_assign!(num_dropped_on_capacity, 1);
                }
                saturating_add_assign!(num_buffered, 1);
            }

            // Update metrics for transactions that were dropped.
            let num_dropped_on_sanitization = chunk.len().saturating_sub(post_sanitization_count);
            let num_dropped_on_lock_validation =
                post_sanitization_count.saturating_sub(post_lock_validation_count);
            let num_dropped_on_transaction_checks =
                post_lock_validation_count.saturating_sub(post_transaction_check_count);

            count_metrics.update(|count_metrics| {
                saturating_add_assign!(
                    count_metrics.num_dropped_on_capacity,
                    num_dropped_on_capacity
                );
                saturating_add_assign!(count_metrics.num_buffered, num_buffered);
                saturating_add_assign!(
                    count_metrics.num_dropped_on_sanitization,
                    num_dropped_on_sanitization
                );
                saturating_add_assign!(
                    count_metrics.num_dropped_on_validate_locks,
                    num_dropped_on_lock_validation
                );
                saturating_add_assign!(
                    count_metrics.num_dropped_on_receive_transaction_checks,
                    num_dropped_on_transaction_checks
                );
            });
        }
    }
}

pub(crate) struct TransactionViewReceiveAndBuffer {
    pub receiver: BankingPacketReceiver,
    pub bank_forks: Arc<RwLock<BankForks>>,
}

impl ReceiveAndBuffer for TransactionViewReceiveAndBuffer {
    type Transaction = RuntimeTransaction<ResolvedTransactionView<SharedBytes>>;
    type Container = TransactionViewStateContainer;

    fn receive_and_buffer_packets(
        &mut self,
        container: &mut Self::Container,
        timing_metrics: &mut SchedulerTimingMetrics,
        count_metrics: &mut SchedulerCountMetrics,
        decision: &BufferedPacketsDecision,
    ) -> Result<usize, ()> {
        let (root_bank, working_bank) = {
            let bank_forks = self.bank_forks.read().unwrap();
            let root_bank = bank_forks.root_bank();
            let working_bank = bank_forks.working_bank();
            (root_bank, working_bank)
        };

        // Receive packet batches.
        const TIMEOUT: Duration = Duration::from_millis(10);
        let start = Instant::now();
        let mut num_received = 0;
        let mut received_message = false;

        // If not leader/unknown, do a blocking-receive initially. This lets
        // the thread sleep until a message is received, or until the timeout.
        // Additionally, only sleep if the container is empty.
        if container.is_empty()
            && matches!(
                decision,
                BufferedPacketsDecision::Forward | BufferedPacketsDecision::ForwardAndHold
            )
        {
            // TODO: Is it better to manually sleep instead, avoiding the locking
            //       overhead for wakers? But then risk not waking up when message
            //       received - as long as sleep is somewhat short, this should be
            //       fine.
            match self.receiver.recv_timeout(TIMEOUT) {
                Ok(packet_batch_message) => {
                    received_message = true;
                    num_received += self.handle_packet_batch_message(
                        container,
                        timing_metrics,
                        count_metrics,
                        decision,
                        &root_bank,
                        &working_bank,
                        packet_batch_message,
                    );
                }
                Err(RecvTimeoutError::Timeout) => return Ok(num_received),
                Err(RecvTimeoutError::Disconnected) => {
                    return received_message.then_some(num_received).ok_or(());
                }
            }
        }

        while start.elapsed() < TIMEOUT {
            match self.receiver.try_recv() {
                Ok(packet_batch_message) => {
                    received_message = true;
                    num_received += self.handle_packet_batch_message(
                        container,
                        timing_metrics,
                        count_metrics,
                        decision,
                        &root_bank,
                        &working_bank,
                        packet_batch_message,
                    );
                }
                Err(TryRecvError::Empty) => return Ok(num_received),
                Err(TryRecvError::Disconnected) => {
                    return received_message.then_some(num_received).ok_or(());
                }
            }
        }

        Ok(num_received)
    }
}

impl TransactionViewReceiveAndBuffer {
    /// Return number of received packets.
    fn handle_packet_batch_message(
        &mut self,
        container: &mut TransactionViewStateContainer,
        timing_metrics: &mut SchedulerTimingMetrics,
        count_metrics: &mut SchedulerCountMetrics,
        decision: &BufferedPacketsDecision,
        root_bank: &Bank,
        working_bank: &Bank,
        packet_batch_message: BankingPacketBatch,
    ) -> usize {
        // Do not support forwarding - only add support for this if we really need it.
        if matches!(decision, BufferedPacketsDecision::Forward) {
            return 0;
        }

        let start = Instant::now();
        // Sanitize packets, generate IDs, and insert into the container.
        let alt_resolved_slot = root_bank.slot();
        let sanitized_epoch = root_bank.epoch();
        let transaction_account_lock_limit = working_bank.get_transaction_account_lock_limit();

        let mut num_received = 0usize;
        let mut num_buffered = 0usize;
        let mut num_dropped_on_status_age_checks = 0usize;
        let mut num_dropped_on_capacity = 0usize;
        let mut num_dropped_on_receive = 0usize;

        // Create temporary batches of transactions to be age-checked.
        let mut transaction_priority_ids = ArrayVec::<_, EXTRA_CAPACITY>::new();
        let lock_results: [_; EXTRA_CAPACITY] = core::array::from_fn(|_| Ok(()));
        let mut error_counters = TransactionErrorMetrics::default();

        let mut check_and_push_to_queue =
            |container: &mut TransactionViewStateContainer,
             transaction_priority_ids: &mut ArrayVec<TransactionPriorityId, 64>| {
                // Temporary scope so that transaction references are immediately
                // dropped and transactions not passing
                let mut check_results = {
                    let mut transactions = ArrayVec::<_, EXTRA_CAPACITY>::new();
                    transactions.extend(transaction_priority_ids.iter().map(|priority_id| {
                        &container
                            .get_transaction_ttl(priority_id.id)
                            .expect("transaction must exist")
                            .transaction
                    }));
                    working_bank.check_transactions::<RuntimeTransaction<_>>(
                        &transactions,
                        &lock_results[..transactions.len()],
                        MAX_PROCESSING_AGE,
                        &mut error_counters,
                    )
                };

                // Remove errored transactions
                for (result, priority_id) in check_results
                    .iter_mut()
                    .zip(transaction_priority_ids.iter())
                {
                    if result.is_err() {
                        num_dropped_on_status_age_checks += 1;
                        container.remove_by_id(priority_id.id);
                    }
                    let transaction = &container
                        .get_transaction_ttl(priority_id.id)
                        .expect("transaction must exist")
                        .transaction;
                    if let Err(err) = Consumer::check_fee_payer_unlocked(
                        working_bank,
                        transaction,
                        &mut error_counters,
                    ) {
                        *result = Err(err);
                        num_dropped_on_status_age_checks += 1;
                        container.remove_by_id(priority_id.id);
                    }
                }
                // Push non-errored transaction into queue.
                num_dropped_on_capacity += container.push_ids_into_queue(
                    check_results
                        .into_iter()
                        .zip(transaction_priority_ids.drain(..))
                        .filter(|(r, _)| r.is_ok())
                        .map(|(_, id)| id),
                );
            };

        for packet_batch in packet_batch_message.iter() {
            for packet in packet_batch.iter() {
                let Some(packet_data) = packet.data(..) else {
                    continue;
                };

                num_received += 1;

                // Reserve free-space to copy packet into, run sanitization checks, and insert.
                if let Some(transaction_id) =
                    container.try_insert_map_only_with_data(packet_data, |bytes| {
                        match Self::try_handle_packet(
                            bytes,
                            root_bank,
                            working_bank,
                            alt_resolved_slot,
                            sanitized_epoch,
                            transaction_account_lock_limit,
                        ) {
                            Ok(state) => {
                                num_buffered += 1;
                                Ok(state)
                            }
                            Err(()) => {
                                num_dropped_on_receive += 1;
                                Err(())
                            }
                        }
                    })
                {
                    let priority = container
                        .get_mut_transaction_state(transaction_id)
                        .expect("transaction must exist")
                        .priority();
                    transaction_priority_ids
                        .push(TransactionPriorityId::new(priority, transaction_id));

                    // If at capacity, run checks and remove invalid transactions.
                    if transaction_priority_ids.len() == EXTRA_CAPACITY {
                        check_and_push_to_queue(container, &mut transaction_priority_ids);
                    }
                }
            }
        }

        // Any remaining packets undergo status/age checks
        check_and_push_to_queue(container, &mut transaction_priority_ids);

        let buffer_time_us = start.elapsed().as_micros() as u64;
        timing_metrics.update(|timing_metrics| {
            saturating_add_assign!(timing_metrics.buffer_time_us, buffer_time_us);
        });
        count_metrics.update(|count_metrics| {
            saturating_add_assign!(count_metrics.num_received, num_received);
            saturating_add_assign!(count_metrics.num_buffered, num_buffered);
            saturating_add_assign!(
                count_metrics.num_dropped_on_age_and_status,
                num_dropped_on_status_age_checks
            );
            saturating_add_assign!(
                count_metrics.num_dropped_on_capacity,
                num_dropped_on_capacity
            );
            saturating_add_assign!(count_metrics.num_dropped_on_receive, num_dropped_on_receive);
        });

        num_received
    }

    fn try_handle_packet(
        bytes: SharedBytes,
        root_bank: &Bank,
        working_bank: &Bank,
        alt_resolved_slot: Slot,
        sanitized_epoch: Epoch,
        transaction_account_lock_limit: usize,
    ) -> Result<TransactionViewState, ()> {
        // Parsing and basic sanitization checks
        let Ok(view) = SanitizedTransactionView::try_new_sanitized(bytes) else {
            return Err(());
        };

        let Ok(view) = RuntimeTransaction::<SanitizedTransactionView<_>>::try_from(
            view,
            MessageHash::Compute,
            None,
        ) else {
            return Err(());
        };

        // Discard non-vote packets if in vote-only mode.
        if root_bank.vote_only_bank() && !view.is_simple_vote_transaction() {
            return Err(());
        }

        // Check excessive pre-compiles.
        let signature_details = view.signature_details();
        let num_precompiles = signature_details.num_ed25519_instruction_signatures()
            + signature_details.num_secp256k1_instruction_signatures()
            + signature_details.num_secp256r1_instruction_signatures();
        if num_precompiles > MAX_ALLOWED_PRECOMPILE_SIGNATURES {
            return Err(());
        }

        // Load addresses for transaction.
        let load_addresses_result = match view.version() {
            TransactionVersion::Legacy => Ok((None, u64::MAX)),
            TransactionVersion::V0 => root_bank
                .load_addresses_from_ref(view.address_table_lookup_iter())
                .map(|(loaded_addresses, deactivation_slot)| {
                    (Some(loaded_addresses), deactivation_slot)
                }),
        };
        let Ok((loaded_addresses, deactivation_slot)) = load_addresses_result else {
            return Err(());
        };

        let Ok(view) = RuntimeTransaction::<ResolvedTransactionView<_>>::try_from(
            view,
            loaded_addresses,
            root_bank.get_reserved_account_keys(),
        ) else {
            return Err(());
        };

        if validate_account_locks(view.account_keys(), transaction_account_lock_limit).is_err() {
            return Err(());
        }

        let Ok(compute_budget_limits) = view
            .compute_budget_instruction_details()
            .sanitize_and_convert_to_compute_budget_limits(&working_bank.feature_set)
        else {
            return Err(());
        };

        let max_age = calculate_max_age(sanitized_epoch, deactivation_slot, alt_resolved_slot);
        let fee_budget_limits = FeeBudgetLimits::from(compute_budget_limits);
        let (priority, cost) = calculate_priority_and_cost(&view, &fee_budget_limits, working_bank);

        Ok(TransactionState::new(
            SanitizedTransactionTTL {
                transaction: view,
                max_age,
            },
            None,
            priority,
            cost,
        ))
    }
}

/// Calculate priority and cost for a transaction:
///
/// Cost is calculated through the `CostModel`,
/// and priority is calculated through a formula here that attempts to sell
/// blockspace to the highest bidder.
///
/// The priority is calculated as:
/// P = R / (1 + C)
/// where P is the priority, R is the reward,
/// and C is the cost towards block-limits.
///
/// Current minimum costs are on the order of several hundred,
/// so the denominator is effectively C, and the +1 is simply
/// to avoid any division by zero due to a bug - these costs
/// are calculated by the cost-model and are not direct
/// from user input. They should never be zero.
/// Any difference in the prioritization is negligible for
/// the current transaction costs.
fn calculate_priority_and_cost(
    transaction: &impl TransactionWithMeta,
    fee_budget_limits: &FeeBudgetLimits,
    bank: &Bank,
) -> (u64, u64) {
    let cost = CostModel::calculate_cost(transaction, &bank.feature_set).sum();
    let reward = bank.calculate_reward_for_transaction(transaction, fee_budget_limits);

    // We need a multiplier here to avoid rounding down too aggressively.
    // For many transactions, the cost will be greater than the fees in terms of raw lamports.
    // For the purposes of calculating prioritization, we multiply the fees by a large number so that
    // the cost is a small fraction.
    // An offset of 1 is used in the denominator to explicitly avoid division by zero.
    const MULTIPLIER: u64 = 1_000_000;
    (
        reward
            .saturating_mul(MULTIPLIER)
            .saturating_div(cost.saturating_add(1)),
        cost,
    )
}

/// Given the epoch, the minimum deactivation slot, and the current slot,
/// return the `MaxAge` that should be used for the transaction. This is used
/// to determine the maximum slot that a transaction will be considered valid
/// for, without re-resolving addresses or resanitizing.
///
/// This function considers the deactivation period of Address Table
/// accounts. If the deactivation period runs past the end of the epoch,
/// then the transaction is considered valid until the end of the epoch.
/// Otherwise, the transaction is considered valid until the deactivation
/// period.
///
/// Since the deactivation period technically uses blocks rather than
/// slots, the value used here is the lower-bound on the deactivation
/// period, i.e. the transaction's address lookups are valid until
/// AT LEAST this slot.
fn calculate_max_age(
    sanitized_epoch: Epoch,
    deactivation_slot: Slot,
    current_slot: Slot,
) -> MaxAge {
    let alt_min_expire_slot = estimate_last_valid_slot(deactivation_slot.min(current_slot));
    MaxAge {
        sanitized_epoch,
        alt_invalidation_slot: alt_min_expire_slot,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_max_age() {
        let current_slot = 100;
        let sanitized_epoch = 10;

        // ALT deactivation slot is delayed
        assert_eq!(
            calculate_max_age(sanitized_epoch, current_slot - 1, current_slot),
            MaxAge {
                sanitized_epoch,
                alt_invalidation_slot: current_slot - 1
                    + solana_sdk::slot_hashes::get_entries() as u64,
            }
        );

        // no deactivation slot
        assert_eq!(
            calculate_max_age(sanitized_epoch, u64::MAX, current_slot),
            MaxAge {
                sanitized_epoch,
                alt_invalidation_slot: current_slot + solana_sdk::slot_hashes::get_entries() as u64,
            }
        );
    }
}
