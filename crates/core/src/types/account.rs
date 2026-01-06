//! Account types for the perpetual DEX

use serde::{Deserialize, Serialize};
use super::{AccountId, AssetId, PublicKey, Timestamp};

/// Account type: main account or sub-account
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AccountType {
    /// Main account (can have sub-accounts)
    #[default]
    Main,
    /// Sub-account linked to a main account
    SubAccount {
        /// The main account this sub-account belongs to
        main_account_id: AccountId,
    },
}

/// Balance for a specific asset
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Balance {
    /// Asset identifier
    pub asset_id: AssetId,
    /// Available (free) balance
    pub free: u128,
    /// Locked balance (in open orders or positions)
    pub locked: u128,
}

impl Balance {
    /// Create a new balance
    pub fn new(asset_id: AssetId, free: u128) -> Self {
        Self {
            asset_id,
            free,
            locked: 0,
        }
    }

    /// Total balance (free + locked)
    pub fn total(&self) -> u128 {
        self.free + self.locked
    }

    /// Lock an amount from free balance
    pub fn lock(&mut self, amount: u128) -> bool {
        if self.free >= amount {
            self.free -= amount;
            self.locked += amount;
            true
        } else {
            false
        }
    }

    /// Unlock an amount back to free balance
    pub fn unlock(&mut self, amount: u128) -> bool {
        if self.locked >= amount {
            self.locked -= amount;
            self.free += amount;
            true
        } else {
            false
        }
    }

    /// Deduct from locked balance (e.g., after fill)
    pub fn deduct_locked(&mut self, amount: u128) -> bool {
        if self.locked >= amount {
            self.locked -= amount;
            true
        } else {
            false
        }
    }

    /// Add to free balance (e.g., deposit or profit)
    pub fn credit(&mut self, amount: u128) {
        self.free += amount;
    }

    /// Deduct from free balance (e.g., withdrawal)
    pub fn debit(&mut self, amount: u128) -> bool {
        if self.free >= amount {
            self.free -= amount;
            true
        } else {
            false
        }
    }
}

/// A user account
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    /// Unique account ID
    pub id: AccountId,
    /// Public key of the account owner (for signature verification)
    pub owner: PublicKey,
    /// Nonce for replay protection (increments with each tx)
    pub nonce: u64,
    /// Account type
    pub account_type: AccountType,
    /// Balances for each asset
    pub balances: Vec<Balance>,
    /// Whether account is flagged for liquidation
    pub is_liquidatable: bool,
    /// Timestamp when account was created
    pub created_at: Timestamp,
}

impl Default for Account {
    fn default() -> Self {
        Self {
            id: 0,
            owner: [0u8; 32],
            nonce: 0,
            account_type: AccountType::Main,
            balances: Vec::new(),
            is_liquidatable: false,
            created_at: 0,
        }
    }
}

impl Account {
    /// Create a new main account
    pub fn new(id: AccountId, owner: PublicKey, created_at: Timestamp) -> Self {
        Self {
            id,
            owner,
            nonce: 0,
            account_type: AccountType::Main,
            balances: Vec::new(),
            is_liquidatable: false,
            created_at,
        }
    }

    /// Get balance for a specific asset
    pub fn get_balance(&self, asset_id: AssetId) -> Option<&Balance> {
        self.balances.iter().find(|b| b.asset_id == asset_id)
    }

    /// Get mutable balance for a specific asset
    pub fn get_balance_mut(&mut self, asset_id: AssetId) -> Option<&mut Balance> {
        self.balances.iter_mut().find(|b| b.asset_id == asset_id)
    }

    /// Get or create balance for an asset
    pub fn get_or_create_balance(&mut self, asset_id: AssetId) -> &mut Balance {
        if !self.balances.iter().any(|b| b.asset_id == asset_id) {
            self.balances.push(Balance::new(asset_id, 0));
        }
        self.balances.iter_mut().find(|b| b.asset_id == asset_id).unwrap()
    }

    /// Get free balance for an asset
    pub fn free_balance(&self, asset_id: AssetId) -> u128 {
        self.get_balance(asset_id).map(|b| b.free).unwrap_or(0)
    }

    /// Get locked balance for an asset
    pub fn locked_balance(&self, asset_id: AssetId) -> u128 {
        self.get_balance(asset_id).map(|b| b.locked).unwrap_or(0)
    }

    /// Deposit funds to account
    pub fn deposit(&mut self, asset_id: AssetId, amount: u128) {
        let balance = self.get_or_create_balance(asset_id);
        balance.credit(amount);
    }

    /// Withdraw funds from account (only free balance)
    pub fn withdraw(&mut self, asset_id: AssetId, amount: u128) -> bool {
        if let Some(balance) = self.get_balance_mut(asset_id) {
            balance.debit(amount)
        } else {
            false
        }
    }

    /// Increment nonce (call after successful transaction)
    pub fn increment_nonce(&mut self) {
        self.nonce += 1;
    }

    /// Verify and consume nonce
    pub fn verify_nonce(&self, expected_nonce: u64) -> bool {
        expected_nonce == self.nonce + 1
    }
}

/// Predefined asset IDs
pub mod assets {
    use super::AssetId;

    /// USDC stablecoin (quote asset)
    pub const USDC: AssetId = 0;
    /// Bitcoin
    pub const BTC: AssetId = 1;
    /// Ethereum
    pub const ETH: AssetId = 2;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_balance_operations() {
        let mut balance = Balance::new(assets::USDC, 1000);

        assert_eq!(balance.free, 1000);
        assert_eq!(balance.locked, 0);
        assert_eq!(balance.total(), 1000);

        // Lock some balance
        assert!(balance.lock(300));
        assert_eq!(balance.free, 700);
        assert_eq!(balance.locked, 300);

        // Can't lock more than free
        assert!(!balance.lock(800));

        // Unlock
        assert!(balance.unlock(100));
        assert_eq!(balance.free, 800);
        assert_eq!(balance.locked, 200);

        // Deduct from locked
        assert!(balance.deduct_locked(50));
        assert_eq!(balance.locked, 150);
    }

    #[test]
    fn test_account_deposit_withdraw() {
        let mut account = Account::new(1, [0u8; 32], 0);

        // Deposit
        account.deposit(assets::USDC, 1000);
        assert_eq!(account.free_balance(assets::USDC), 1000);

        // Withdraw
        assert!(account.withdraw(assets::USDC, 300));
        assert_eq!(account.free_balance(assets::USDC), 700);

        // Can't withdraw more than balance
        assert!(!account.withdraw(assets::USDC, 800));
    }

    #[test]
    fn test_account_nonce() {
        let mut account = Account::new(1, [0u8; 32], 0);

        assert_eq!(account.nonce, 0);
        assert!(account.verify_nonce(1));
        assert!(!account.verify_nonce(0));
        assert!(!account.verify_nonce(2));

        account.increment_nonce();
        assert_eq!(account.nonce, 1);
        assert!(account.verify_nonce(2));
    }
}
