//! Container to capture information relevant to computing a bank hash

use {
    super::Bank,
    base64::{prelude::BASE64_STANDARD, Engine},
    log::*,
    serde::{
        de::{self, Deserialize, Deserializer},
        ser::{Serialize, SerializeSeq, Serializer},
    },
    solana_accounts_db::{
        accounts_db::PubkeyHashAccount,
        accounts_hash::{AccountHash, AccountsDeltaHash},
    },
    solana_sdk::{
        account::{Account, AccountSharedData, ReadableAccount},
        clock::{Epoch, Slot},
        fee::FeeDetails,
        hash::Hash,
        inner_instruction::InnerInstructionsList,
        pubkey::Pubkey,
        transaction::Result as TransactionResult,
        transaction_context::TransactionReturnData,
    },
    solana_svm::transaction_commit_result::CommittedTransaction,
    solana_transaction_status::UiInstruction,
    std::str::FromStr,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BankHashDetails {
    /// The client version
    pub version: String,
    /// The encoding format for account data buffers
    pub account_data_encoding: String,
    /// Bank hash details for a collection of banks
    pub bank_hash_details: Vec<SlotDetails>,
}

impl BankHashDetails {
    pub fn new(bank_hash_details: Vec<SlotDetails>) -> Self {
        Self {
            version: solana_version::version!().to_string(),
            account_data_encoding: "base64".to_string(),
            bank_hash_details,
        }
    }

    /// Determines a filename given the currently held bank details
    pub fn filename(&self) -> Result<String, String> {
        if self.bank_hash_details.is_empty() {
            return Err("BankHashDetails does not contains details for any banks".to_string());
        }
        // From here on, .unwrap() on .first() and .second() is safe as
        // self.bank_hash_details is known to be non-empty
        let (first_slot, first_hash) = {
            let details = self.bank_hash_details.first().unwrap();
            (details.slot, &details.bank_hash)
        };

        let filename = if self.bank_hash_details.len() == 1 {
            format!("{first_slot}-{first_hash}.json")
        } else {
            let (last_slot, last_hash) = {
                let details = self.bank_hash_details.last().unwrap();
                (details.slot, &details.bank_hash)
            };
            format!("{first_slot}-{first_hash}_{last_slot}-{last_hash}.json")
        };
        Ok(filename)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Default)]
pub struct TransactionDetails {
    pub signature: String,
    pub index: usize,
    pub accounts: Vec<String>,
    pub instructions: Vec<UiInstruction>,
    pub is_simple_vote_tx: bool,
    pub commit_details: Option<TransactionCommitDetails>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TransactionCommitDetails {
    pub status: TransactionResult<()>,
    pub log_messages: Option<Vec<String>>,
    pub inner_instructions: Option<InnerInstructionsList>,
    pub return_data: Option<TransactionReturnData>,
    pub executed_units: u64,
    pub fee_details: FeeDetails,
}

impl From<CommittedTransaction> for TransactionCommitDetails {
    fn from(committed_tx: CommittedTransaction) -> Self {
        Self {
            status: committed_tx.status,
            log_messages: committed_tx.log_messages,
            inner_instructions: committed_tx.inner_instructions,
            return_data: committed_tx.return_data,
            executed_units: committed_tx.executed_units,
            fee_details: committed_tx.fee_details,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Default)]
pub struct SlotDetails {
    pub slot: Slot,
    pub bank_hash: String,
    #[serde(skip_serializing_if = "Option::is_none", default, flatten)]
    pub bank_hash_components: Option<BankHashComponents>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub transactions: Vec<TransactionDetails>,
}

/// The components that go into a bank hash calculation for a single bank
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Default)]
pub struct BankHashComponents {
    pub parent_bank_hash: String,
    pub accounts_delta_hash: String,
    pub signature_count: u64,
    pub last_blockhash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epoch_accounts_hash: Option<String>,
    pub accounts: AccountsDetails,
}

impl SlotDetails {
    pub fn new_from_bank(bank: &Bank, include_bank_hash_components: bool) -> Result<Self, String> {
        let slot = bank.slot();
        if !bank.is_frozen() {
            return Err(format!(
                "Bank {slot} must be frozen in order to get bank hash details"
            ));
        }

        let bank_hash_components = if include_bank_hash_components {
            // This bank is frozen; as a result, we know that the state has been
            // hashed which means the delta hash is Some(). So, .unwrap() is safe
            let AccountsDeltaHash(accounts_delta_hash) = bank
                .rc
                .accounts
                .accounts_db
                .get_accounts_delta_hash(slot)
                .unwrap();
            let accounts = bank.get_accounts_for_bank_hash_details();

            Some(BankHashComponents {
                parent_bank_hash: bank.parent_hash().to_string(),
                accounts_delta_hash: accounts_delta_hash.to_string(),
                signature_count: bank.signature_count(),
                last_blockhash: bank.last_blockhash().to_string(),
                // The bank is already frozen so this should not have to wait
                epoch_accounts_hash: bank
                    .wait_get_epoch_accounts_hash()
                    .map(|hash| hash.as_ref().to_string()),
                accounts: AccountsDetails { accounts },
            })
        } else {
            None
        };

        Ok(Self {
            slot,
            bank_hash: bank.hash().to_string(),
            bank_hash_components,
            transactions: Vec::new(),
        })
    }
}

/// Wrapper around a Vec<_> to facilitate custom Serialize/Deserialize trait
/// implementations.
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct AccountsDetails {
    pub accounts: Vec<PubkeyHashAccount>,
}

/// Used as an intermediate for serializing and deserializing account fields
/// into a human readable format.
#[derive(Deserialize, Serialize)]
struct SerdeAccount {
    pubkey: String,
    hash: String,
    owner: String,
    lamports: u64,
    rent_epoch: Epoch,
    executable: bool,
    data: String,
}

impl From<&PubkeyHashAccount> for SerdeAccount {
    fn from(pubkey_hash_account: &PubkeyHashAccount) -> Self {
        let PubkeyHashAccount {
            pubkey,
            hash,
            account,
        } = pubkey_hash_account;
        Self {
            pubkey: pubkey.to_string(),
            hash: hash.0.to_string(),
            owner: account.owner().to_string(),
            lamports: account.lamports(),
            rent_epoch: account.rent_epoch(),
            executable: account.executable(),
            data: BASE64_STANDARD.encode(account.data()),
        }
    }
}

impl TryFrom<SerdeAccount> for PubkeyHashAccount {
    type Error = String;

    fn try_from(temp_account: SerdeAccount) -> Result<Self, Self::Error> {
        let pubkey = Pubkey::from_str(&temp_account.pubkey).map_err(|err| err.to_string())?;
        let hash = AccountHash(Hash::from_str(&temp_account.hash).map_err(|err| err.to_string())?);

        let account = AccountSharedData::from(Account {
            lamports: temp_account.lamports,
            data: BASE64_STANDARD
                .decode(temp_account.data)
                .map_err(|err| err.to_string())?,
            owner: Pubkey::from_str(&temp_account.owner).map_err(|err| err.to_string())?,
            executable: temp_account.executable,
            rent_epoch: temp_account.rent_epoch,
        });

        Ok(Self {
            pubkey,
            hash,
            account,
        })
    }
}

impl Serialize for AccountsDetails {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(self.accounts.len()))?;
        for account in self.accounts.iter() {
            let temp_account = SerdeAccount::from(account);
            seq.serialize_element(&temp_account)?;
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for AccountsDetails {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let temp_accounts: Vec<SerdeAccount> = Deserialize::deserialize(deserializer)?;
        let pubkey_hash_accounts: Result<Vec<_>, _> = temp_accounts
            .into_iter()
            .map(PubkeyHashAccount::try_from)
            .collect();
        let pubkey_hash_accounts = pubkey_hash_accounts.map_err(de::Error::custom)?;
        Ok(AccountsDetails {
            accounts: pubkey_hash_accounts,
        })
    }
}

/// Output the components that comprise the overall bank hash for the supplied `Bank`
pub fn write_bank_hash_details_file(bank: &Bank) -> std::result::Result<(), String> {
    let slot_details = SlotDetails::new_from_bank(bank, /*include_bank_hash_mixins:*/ true)?;
    let details = BankHashDetails::new(vec![slot_details]);

    let parent_dir = bank
        .rc
        .accounts
        .accounts_db
        .get_base_working_path()
        .join("bank_hash_details");
    let path = parent_dir.join(details.filename()?);
    // A file with the same name implies the same hash for this slot. Skip
    // rewriting a duplicate file in this scenario
    if !path.exists() {
        info!("writing bank hash details file: {}", path.display());

        // std::fs::write may fail (depending on platform) if the full directory
        // path does not exist. So, call std::fs_create_dir_all first.
        // https://doc.rust-lang.org/std/fs/fn.write.html
        _ = std::fs::create_dir_all(parent_dir);
        let file = std::fs::File::create(&path)
            .map_err(|err| format!("Unable to create file at {}: {err}", path.display()))?;

        // writing the json file ends up with a syscall for each number, comma, indentation etc.
        // use BufWriter to speed things up
        let writer = std::io::BufWriter::new(file);

        serde_json::to_writer_pretty(writer, &details)
            .map_err(|err| format!("Unable to write file at {}: {err}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::*;

    fn build_details(num_slots: usize) -> BankHashDetails {
        let slot_details: Vec<_> = (0..num_slots)
            .map(|slot| {
                let slot = slot as u64;

                let account = AccountSharedData::from(Account {
                    lamports: 123_456_789,
                    data: vec![0, 9, 1, 8, 2, 7, 3, 6, 4, 5],
                    owner: Pubkey::new_unique(),
                    executable: true,
                    rent_epoch: 123,
                });
                let account_pubkey = Pubkey::new_unique();
                let account_hash = AccountHash(solana_sdk::hash::hash("account".as_bytes()));
                let accounts = AccountsDetails {
                    accounts: vec![PubkeyHashAccount {
                        pubkey: account_pubkey,
                        hash: account_hash,
                        account,
                    }],
                };

                SlotDetails {
                    slot,
                    bank_hash: format!("bank{slot}"),
                    bank_hash_components: Some(BankHashComponents {
                        parent_bank_hash: "parent_bank_hash".into(),
                        accounts_delta_hash: "accounts_delta_hash".into(),
                        signature_count: slot + 10,
                        last_blockhash: "last_blockhash".into(),
                        epoch_accounts_hash: if slot % 2 == 0 {
                            Some("epoch_accounts_hash".into())
                        } else {
                            None
                        },
                        accounts,
                    }),
                    transactions: vec![],
                }
            })
            .collect();

        BankHashDetails::new(slot_details)
    }

    #[test]
    fn test_serde_bank_hash_details() {
        let num_slots = 10;
        let bank_hash_details = build_details(num_slots);

        let serialized_bytes = serde_json::to_vec(&bank_hash_details).unwrap();
        let deserialized_bank_hash_details: BankHashDetails =
            serde_json::from_slice(&serialized_bytes).unwrap();

        assert_eq!(bank_hash_details, deserialized_bank_hash_details);
    }
}
