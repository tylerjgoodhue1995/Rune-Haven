pub const VGLD_NAME: &str = "Veloren Gold";
pub const VGLD_SYMBOL: &str = "VGLD";
pub const VGLD_DECIMALS: u8 = 9;

use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LedgerError {
    InvalidAccount,
    InvalidAmount,
    InsufficientBalance,
    DuplicateTransaction,
    MintMismatch,
    WalletMismatch,
    WithdrawalsDisabled,
    UnauthorizedWithdrawal,
    PersistenceFailed,
    DepositVerificationFailed,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct VgldLedger {
    balances: HashMap<String, u64>,
    applied_transactions: HashSet<String>,
    #[serde(skip)]
    persistence_path: Option<PathBuf>,
}

impl VgldLedger {
    pub fn with_persistence_path(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut ledger = Self {
            persistence_path: Some(path.clone()),
            ..Self::default()
        };
        if path.exists() {
            match fs::read(&path)
                .map_err(|_| ())
                .and_then(|bytes| serde_json::from_slice::<VgldLedger>(&bytes).map_err(|_| ()))
            {
                Ok(mut loaded) => {
                    loaded.persistence_path = Some(path);
                    ledger = loaded;
                },
                Err(()) => {
                    tracing::error!(path = %path.display(), "failed to load VGLD ledger");
                },
            }
        }
        ledger
    }

    pub fn balance(&self, account: &str) -> u64 {
        self.balances.get(account).copied().unwrap_or_default()
    }

    pub fn credit(
        &mut self,
        account: &str,
        amount: u64,
        transaction_id: &str,
    ) -> Result<(), LedgerError> {
        self.validate_inputs(account, amount, transaction_id)?;
        let previous = self.clone();
        if !self.applied_transactions.insert(transaction_id.to_owned()) {
            return Err(LedgerError::DuplicateTransaction);
        }
        let balance = self.balances.entry(account.to_owned()).or_default();
        *balance = match balance.checked_add(amount) {
            Some(balance) => balance,
            None => {
                *self = previous;
                return Err(LedgerError::InvalidAmount);
            },
        };
        if let Err(error) = self.persist() {
            *self = previous;
            return Err(error);
        }
        Ok(())
    }

    pub fn debit(
        &mut self,
        account: &str,
        amount: u64,
        transaction_id: &str,
    ) -> Result<(), LedgerError> {
        self.validate_inputs(account, amount, transaction_id)?;
        if self.balance(account) < amount {
            return Err(LedgerError::InsufficientBalance);
        }
        let previous = self.clone();
        if !self.applied_transactions.insert(transaction_id.to_owned()) {
            return Err(LedgerError::DuplicateTransaction);
        }
        let balance = self.balances.get_mut(account).expect("balance was checked");
        *balance -= amount;
        if let Err(error) = self.persist() {
            *self = previous;
            return Err(error);
        }
        Ok(())
    }

    fn persist(&self) -> Result<(), LedgerError> {
        let Some(path) = &self.persistence_path else {
            return Ok(());
        };
        let bytes = serde_json::to_vec_pretty(self).map_err(|_| LedgerError::PersistenceFailed)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|_| LedgerError::PersistenceFailed)?;
        }
        let temporary_path = path.with_extension("tmp");
        fs::write(&temporary_path, bytes).map_err(|_| LedgerError::PersistenceFailed)?;
        fs::rename(&temporary_path, path).map_err(|_| LedgerError::PersistenceFailed)
    }

    fn validate_inputs(
        &self,
        account: &str,
        amount: u64,
        transaction_id: &str,
    ) -> Result<(), LedgerError> {
        if account.trim().is_empty() {
            return Err(LedgerError::InvalidAccount);
        }
        if amount == 0 || transaction_id.trim().is_empty() {
            return Err(LedgerError::InvalidAmount);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedDeposit {
    pub transaction_id: String,
    pub wallet: String,
    pub mint: String,
    pub amount: u64,
}

pub trait DepositVerifier {
    fn verify_deposit(
        &self,
        transaction_id: &str,
        wallet: &str,
        mint: &str,
        treasury: Option<&str>,
    ) -> Result<VerifiedDeposit, LedgerError>;
}

pub struct SolanaDepositVerifier {
    rpc_url: String,
    http: reqwest::blocking::Client,
}

impl SolanaDepositVerifier {
    pub fn new(rpc_url: impl Into<String>) -> Self {
        Self {
            rpc_url: rpc_url.into(),
            http: reqwest::blocking::Client::new(),
        }
    }

    fn parse_verified_deposit(
        transaction_id: &str,
        wallet: &str,
        mint: &str,
        treasury: Option<&str>,
        response: serde_json::Value,
    ) -> Result<VerifiedDeposit, LedgerError> {
        let result = response
            .get("result")
            .ok_or(LedgerError::DepositVerificationFailed)?;
        if result.is_null()
            || result
                .get("meta")
                .and_then(|meta| meta.get("err"))
                .is_some_and(|err| !err.is_null())
            || result.get("meta").is_none()
        {
            return Err(LedgerError::DepositVerificationFailed);
        }
        if result
            .get("confirmationStatus")
            .and_then(serde_json::Value::as_str)
            != Some("finalized")
        {
            return Err(LedgerError::DepositVerificationFailed);
        }

        let mut sent = 0u64;
        let mut received = 0u64;
        let balances = result
            .get("meta")
            .and_then(|meta| meta.get("postTokenBalances"))
            .and_then(serde_json::Value::as_array)
            .ok_or(LedgerError::DepositVerificationFailed)?;
        let pre_balances = result
            .get("meta")
            .and_then(|meta| meta.get("preTokenBalances"))
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();

        for post in balances {
            if post.get("mint").and_then(serde_json::Value::as_str) != Some(mint) {
                continue;
            }
            let post_amount = token_amount(post)?;
            let pre_amount = pre_balances
                .iter()
                .find(|pre| pre.get("accountIndex") == post.get("accountIndex"))
                .map(token_amount)
                .transpose()?
                .unwrap_or_default();
            let delta = post_amount as i128 - pre_amount as i128;
            if post.get("owner").and_then(serde_json::Value::as_str) == Some(wallet) {
                if delta >= 0 {
                    continue;
                }
                sent = sent
                    .checked_add((-delta) as u64)
                    .ok_or(LedgerError::DepositVerificationFailed)?;
            } else if treasury.is_some_and(|owner| {
                post.get("owner").and_then(serde_json::Value::as_str) == Some(owner)
            }) && delta > 0
            {
                received = received
                    .checked_add(delta as u64)
                    .ok_or(LedgerError::DepositVerificationFailed)?;
            }
        }

        if sent == 0 || sent != received || treasury.is_none() {
            return Err(LedgerError::DepositVerificationFailed);
        }
        Ok(VerifiedDeposit {
            transaction_id: transaction_id.to_owned(),
            wallet: wallet.to_owned(),
            mint: mint.to_owned(),
            amount: received,
        })
    }
}

fn token_amount(balance: &serde_json::Value) -> Result<u64, LedgerError> {
    balance
        .get("uiTokenAmount")
        .and_then(|amount| amount.get("amount"))
        .and_then(serde_json::Value::as_str)
        .and_then(|amount| amount.parse().ok())
        .ok_or(LedgerError::DepositVerificationFailed)
}

impl DepositVerifier for SolanaDepositVerifier {
    fn verify_deposit(
        &self,
        transaction_id: &str,
        wallet: &str,
        mint: &str,
        treasury: Option<&str>,
    ) -> Result<VerifiedDeposit, LedgerError> {
        if bs58::decode(transaction_id)
            .into_vec()
            .map_or(true, |bytes| bytes.len() != 64)
        {
            return Err(LedgerError::DepositVerificationFailed);
        }

        let response = self
            .http
            .post(&self.rpc_url)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getTransaction",
                "params": [
                    transaction_id,
                    {
                        "commitment": "finalized",
                        "encoding": "jsonParsed",
                        "maxSupportedTransactionVersion": 0
                    }
                ]
            }))
            .send()
            .map_err(|_| LedgerError::DepositVerificationFailed)?
            .error_for_status()
            .map_err(|_| LedgerError::DepositVerificationFailed)?
            .json::<serde_json::Value>()
            .map_err(|_| LedgerError::DepositVerificationFailed)?;

        Self::parse_verified_deposit(transaction_id, wallet, mint, treasury, response)
    }
}

pub fn apply_verified_deposit(
    ledger: &mut VgldLedger,
    account: &str,
    expected_wallet: &str,
    expected_mint: &str,
    deposit: VerifiedDeposit,
) -> Result<(), LedgerError> {
    if deposit.wallet != expected_wallet {
        return Err(LedgerError::WalletMismatch);
    }
    if deposit.mint != expected_mint {
        return Err(LedgerError::MintMismatch);
    }
    ledger.credit(account, deposit.amount, &deposit.transaction_id)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WithdrawalRequest {
    pub account: String,
    pub wallet: String,
    pub amount: u64,
    pub transaction_id: String,
}

pub trait WithdrawalAuthorizer {
    fn authorize(&self, account: &str, wallet: &str) -> bool;
}

pub fn apply_withdrawal(
    ledger: &mut VgldLedger,
    request: &WithdrawalRequest,
    authorizer: &impl WithdrawalAuthorizer,
    enabled: bool,
) -> Result<(), LedgerError> {
    if !enabled {
        return Err(LedgerError::WithdrawalsDisabled);
    }
    if !authorizer.authorize(&request.account, &request.wallet) {
        return Err(LedgerError::UnauthorizedWithdrawal);
    }
    ledger.debit(&request.account, request.amount, &request.transaction_id)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VgldConfig {
    pub network: String,
    pub rpc_url: String,
    pub mint_address: Option<String>,
    pub treasury_address: Option<String>,
}

impl Default for VgldConfig {
    fn default() -> Self {
        Self {
            network: "devnet".to_string(),
            rpc_url: "https://api.devnet.solana.com".to_string(),
            mint_address: None,
            treasury_address: None,
        }
    }
}

impl VgldConfig {
    pub fn from_env() -> Self {
        Self {
            network: std::env::var("VELOREN_VGLD_NETWORK")
                .or_else(|_| std::env::var("SOLANA_NETWORK"))
                .unwrap_or_else(|_| "devnet".to_string()),
            rpc_url: std::env::var("VELOREN_VGLD_RPC_URL")
                .or_else(|_| std::env::var("SOLANA_RPC_URL"))
                .unwrap_or_else(|_| "https://api.devnet.solana.com".to_string()),
            mint_address: std::env::var("VELOREN_VGLD_MINT")
                .or_else(|_| std::env::var("VGLD_MINT_ADDRESS"))
                .ok(),
            treasury_address: std::env::var("VELOREN_VGLD_TREASURY").ok(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.network.trim().is_empty() {
            return Err("VGLD network must not be empty".to_string());
        }
        if self.rpc_url.trim().is_empty() {
            return Err("VGLD RPC URL must not be empty".to_string());
        }
        if let Some(mint) = &self.mint_address {
            let bytes = bs58::decode(mint)
                .into_vec()
                .map_err(|_| "VGLD mint must be a valid Solana public key".to_string())?;
            if bytes.len() != 32 {
                return Err("VGLD mint must be a valid Solana public key".to_string());
            }
        }
        Ok(())
    }

    pub fn is_configured(&self) -> bool { self.mint_address.is_some() }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestAuthorizer {
        allowed: bool,
    }

    impl WithdrawalAuthorizer for TestAuthorizer {
        fn authorize(&self, _account: &str, _wallet: &str) -> bool { self.allowed }
    }

    #[test]
    fn ledger_credits_and_debits_base_units() {
        let mut ledger = VgldLedger::default();
        ledger
            .credit("player-1", 1_500_000_000, "deposit-1")
            .unwrap();
        ledger.debit("player-1", 500_000_000, "purchase-1").unwrap();
        assert_eq!(ledger.balance("player-1"), 1_000_000_000);
    }

    #[test]
    fn ledger_rejects_duplicate_transactions() {
        let mut ledger = VgldLedger::default();
        ledger.credit("player-1", 10, "deposit-1").unwrap();
        assert_eq!(
            ledger.credit("player-1", 10, "deposit-1"),
            Err(LedgerError::DuplicateTransaction)
        );
    }

    #[test]
    fn ledger_restores_balances_and_transaction_ids() {
        let path =
            std::env::temp_dir().join(format!("veloren-vgld-test-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);

        let mut ledger = VgldLedger::with_persistence_path(&path);
        ledger.credit("player-1", 10, "deposit-1").unwrap();

        let restored = VgldLedger::with_persistence_path(&path);
        assert_eq!(restored.balance("player-1"), 10);
        assert_eq!(
            restored.clone().credit("player-1", 10, "deposit-1"),
            Err(LedgerError::DuplicateTransaction)
        );

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn solana_transaction_parser_accepts_finalized_matching_token_deposit() {
        let response = serde_json::json!({
            "result": {
                "confirmationStatus": "finalized",
                "meta": {
                    "err": null,
                    "preTokenBalances": [{
                        "accountIndex": 2,
                        "owner": "buyer-wallet",
                        "mint": "vgld-mint",
                        "uiTokenAmount": { "amount": "1000000004", "decimals": 9 }
                    }],
                    "postTokenBalances": [{
                        "accountIndex": 2,
                        "owner": "buyer-wallet",
                        "mint": "vgld-mint",
                        "uiTokenAmount": { "amount": "4", "decimals": 9 }
                    }, {
                        "accountIndex": 3,
                        "owner": "treasury-wallet",
                        "mint": "vgld-mint",
                        "uiTokenAmount": { "amount": "1000000000", "decimals": 9 }
                    }]
                }
            }
        });

        let deposit = SolanaDepositVerifier::parse_verified_deposit(
            &"1".repeat(88),
            "buyer-wallet",
            "vgld-mint",
            Some("treasury-wallet"),
            response,
        )
        .unwrap();
        assert_eq!(deposit.amount, 1_000_000_000);
    }

    #[test]
    fn solana_transaction_parser_rejects_unfinalized_or_wrong_mint() {
        let response = serde_json::json!({
            "result": {
                "confirmationStatus": "confirmed",
                "meta": {
                    "err": null,
                    "postTokenBalances": [{
                        "accountIndex": 2,
                        "owner": "buyer-wallet",
                        "mint": "other-mint",
                        "uiTokenAmount": { "amount": "1", "decimals": 9 }
                    }]
                }
            }
        });

        assert_eq!(
            SolanaDepositVerifier::parse_verified_deposit(
                &"1".repeat(88),
                "buyer-wallet",
                "vgld-mint",
                Some("treasury-wallet"),
                response,
            ),
            Err(LedgerError::DepositVerificationFailed)
        );
    }

    #[test]
    fn ledger_rejects_insufficient_balance() {
        let mut ledger = VgldLedger::default();
        assert_eq!(
            ledger.debit("player-1", 1, "purchase-1"),
            Err(LedgerError::InsufficientBalance)
        );
    }

    #[test]
    fn verified_deposit_credits_only_matching_wallet_and_mint() {
        let mut ledger = VgldLedger::default();
        let deposit = VerifiedDeposit {
            transaction_id: "tx-1".to_string(),
            wallet: "wallet-1".to_string(),
            mint: "mint-1".to_string(),
            amount: 2_000_000_000,
        };
        apply_verified_deposit(&mut ledger, "player-1", "wallet-1", "mint-1", deposit).unwrap();
        assert_eq!(ledger.balance("player-1"), 2_000_000_000);
    }

    #[test]
    fn verified_deposit_rejects_wallet_or_mint_mismatch() {
        let deposit = VerifiedDeposit {
            transaction_id: "tx-1".to_string(),
            wallet: "wallet-2".to_string(),
            mint: "mint-2".to_string(),
            amount: 1,
        };
        let mut ledger = VgldLedger::default();
        assert_eq!(
            apply_verified_deposit(&mut ledger, "player-1", "wallet-1", "mint-1", deposit),
            Err(LedgerError::WalletMismatch)
        );
        assert_eq!(ledger.balance("player-1"), 0);
    }

    #[test]
    fn withdrawals_are_disabled_by_default() {
        let mut ledger = VgldLedger::default();
        ledger.credit("player-1", 100, "deposit-1").unwrap();
        let request = WithdrawalRequest {
            account: "player-1".to_string(),
            wallet: "wallet-1".to_string(),
            amount: 50,
            transaction_id: "withdrawal-1".to_string(),
        };
        assert_eq!(
            apply_withdrawal(
                &mut ledger,
                &request,
                &TestAuthorizer { allowed: true },
                false
            ),
            Err(LedgerError::WithdrawalsDisabled)
        );
        assert_eq!(ledger.balance("player-1"), 100);
    }

    #[test]
    fn authorized_withdrawal_debits_once() {
        let mut ledger = VgldLedger::default();
        ledger.credit("player-1", 100, "deposit-1").unwrap();
        let request = WithdrawalRequest {
            account: "player-1".to_string(),
            wallet: "wallet-1".to_string(),
            amount: 50,
            transaction_id: "withdrawal-1".to_string(),
        };
        apply_withdrawal(
            &mut ledger,
            &request,
            &TestAuthorizer { allowed: true },
            true,
        )
        .unwrap();
        assert_eq!(ledger.balance("player-1"), 50);
        assert_eq!(
            apply_withdrawal(
                &mut ledger,
                &request,
                &TestAuthorizer { allowed: true },
                true
            ),
            Err(LedgerError::DuplicateTransaction)
        );
    }

    #[test]
    fn unauthorized_withdrawal_does_not_change_balance() {
        let mut ledger = VgldLedger::default();
        ledger.credit("player-1", 100, "deposit-1").unwrap();
        let request = WithdrawalRequest {
            account: "player-1".to_string(),
            wallet: "wallet-1".to_string(),
            amount: 50,
            transaction_id: "withdrawal-1".to_string(),
        };
        assert_eq!(
            apply_withdrawal(
                &mut ledger,
                &request,
                &TestAuthorizer { allowed: false },
                true
            ),
            Err(LedgerError::UnauthorizedWithdrawal)
        );
        assert_eq!(ledger.balance("player-1"), 100);
    }

    #[test]
    fn defaults_describe_devnet_vgld() {
        let config = VgldConfig::default();
        assert_eq!(VGLD_NAME, "Veloren Gold");
        assert_eq!(VGLD_SYMBOL, "VGLD");
        assert_eq!(VGLD_DECIMALS, 9);
        assert_eq!(config.network, "devnet");
        assert!(!config.is_configured());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn rejects_invalid_mint() {
        let config = VgldConfig {
            mint_address: Some("not-a-mint".to_string()),
            ..VgldConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn accepts_valid_mint() {
        let config = VgldConfig {
            mint_address: Some("11111111111111111111111111111111".to_string()),
            ..VgldConfig::default()
        };
        assert!(config.validate().is_ok());
    }
}
