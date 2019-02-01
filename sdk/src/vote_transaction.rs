//! The `vote_transaction` module provides functionality for creating vote transactions.

use crate::hash::Hash;
use crate::pubkey::Pubkey;
use crate::signature::{Keypair, KeypairUtil};
use crate::system_instruction::SystemInstruction;
use crate::system_program;
use crate::transaction::{Instruction, Transaction};
use crate::vote_program::{self, Vote, VoteInstruction};
use bincode::deserialize;

pub struct VoteTransaction {}

impl VoteTransaction {
    pub fn new_vote<T: KeypairUtil>(
        vote_account: &T,
        tick_height: u64,
        last_id: Hash,
        fee: u64,
    ) -> Transaction {
        let vote = Vote { tick_height };
        let instruction = VoteInstruction::NewVote(vote);
        Transaction::new(
            vote_account,
            &[],
            vote_program::id(),
            &instruction,
            last_id,
            fee,
        )
    }

    pub fn new_account(
        validator_id: &Keypair,
        vote_account_id: Pubkey,
        last_id: Hash,
        num_tokens: u64,
        fee: u64,
    ) -> Transaction {
        Transaction::new_with_instructions(
            &[validator_id],
            &[vote_account_id],
            last_id,
            fee,
            vec![system_program::id(), vote_program::id()],
            vec![
                Instruction::new(
                    0,
                    &SystemInstruction::CreateAccount {
                        tokens: num_tokens,
                        space: vote_program::get_max_size() as u64,
                        program_id: vote_program::id(),
                    },
                    vec![0, 1],
                ),
                Instruction::new(1, &VoteInstruction::RegisterAccount, vec![0, 1]),
            ],
        )
    }

    pub fn get_votes(tx: &Transaction) -> Vec<(Pubkey, Vote, Hash)> {
        let mut votes = vec![];
        for i in 0..tx.instructions.len() {
            let tx_program_id = tx.program_id(i);
            if vote_program::check_id(&tx_program_id) {
                if let Ok(Some(VoteInstruction::NewVote(vote))) = deserialize(&tx.userdata(i)) {
                    votes.push((tx.account_keys[0], vote, tx.last_id))
                }
            }
        }
        votes
    }
}
