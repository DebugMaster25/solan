use solana_vote_api::vote_processor::process_instruction;

solana_sdk::process_instruction_entrypoint!(process_instruction);
