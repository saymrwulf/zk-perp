//! Hypertree: 6-tree composition for full protocol state
//!
//! The hypertree combines 6 Sparse Merkle Trees into a single state commitment:
//! 1. Account Tree - User accounts and balances
//! 2. Account Order Tree - Orders per account
//! 3. Order Book Tree - All active orders (price-time priority encoded in path)
//! 4. Position Tree - Open positions
//! 5. Pool Tree - Liquidity pools
//! 6. System Tree - Global system state and configuration

use serde::{Deserialize, Serialize};
use super::poseidon::{Hash, Sha256Hasher, MerkleHasher, ZERO_HASH};
use super::tree::{SparseMerkleTree, MerkleProof, DEFAULT_TREE_DEPTH};
use crate::types::*;

/// Tree identifiers for the hypertree
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TreeId {
    /// Account Tree (balances, nonce, metadata)
    Account = 0,
    /// Account Order Tree (orders per account)
    AccountOrder = 1,
    /// Order Book Tree (all active orders)
    OrderBook = 2,
    /// Position Tree (open positions)
    Position = 3,
    /// Pool Tree (liquidity pools)
    Pool = 4,
    /// System Tree (global state, markets, oracles)
    System = 5,
}

/// A witness for a state transition affecting multiple trees
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HypertreeWitness {
    /// Pre-state root
    pub pre_root: Hash,
    /// Post-state root
    pub post_root: Hash,
    /// Proofs for each accessed key in each tree
    pub proofs: Vec<(TreeId, MerkleProof)>,
}

/// Keys for system tree entries
pub mod system_keys {
    use super::*;

    /// Key for system state (block number, timestamp, etc.)
    pub fn state() -> Hash {
        let hasher = Sha256Hasher::new();
        hasher.hash(b"system:state")
    }

    /// Key for a market configuration
    pub fn market(market_id: MarketId) -> Hash {
        let hasher = Sha256Hasher::new();
        let mut data = b"system:market:".to_vec();
        data.extend_from_slice(&market_id.to_le_bytes());
        hasher.hash(&data)
    }

    /// Key for oracle price
    pub fn oracle(market_id: MarketId) -> Hash {
        let hasher = Sha256Hasher::new();
        let mut data = b"system:oracle:".to_vec();
        data.extend_from_slice(&market_id.to_le_bytes());
        hasher.hash(&data)
    }
}

/// The 6-tree hypertree structure
#[derive(Clone)]
pub struct Hypertree<H: MerkleHasher + Clone = Sha256Hasher> {
    /// Account Tree
    pub account_tree: SparseMerkleTree<H>,
    /// Account Order Tree
    pub account_order_tree: SparseMerkleTree<H>,
    /// Order Book Tree
    pub orderbook_tree: SparseMerkleTree<H>,
    /// Position Tree
    pub position_tree: SparseMerkleTree<H>,
    /// Pool Tree
    pub pool_tree: SparseMerkleTree<H>,
    /// System Tree
    pub system_tree: SparseMerkleTree<H>,
    /// Hasher for combining roots
    hasher: H,
}

impl<H: MerkleHasher + Clone> Hypertree<H> {
    /// Create a new empty hypertree
    pub fn new(hasher: H) -> Self {
        Self {
            account_tree: SparseMerkleTree::new(hasher.clone(), DEFAULT_TREE_DEPTH),
            account_order_tree: SparseMerkleTree::new(hasher.clone(), DEFAULT_TREE_DEPTH),
            orderbook_tree: SparseMerkleTree::new(hasher.clone(), DEFAULT_TREE_DEPTH),
            position_tree: SparseMerkleTree::new(hasher.clone(), DEFAULT_TREE_DEPTH),
            pool_tree: SparseMerkleTree::new(hasher.clone(), DEFAULT_TREE_DEPTH),
            system_tree: SparseMerkleTree::new(hasher.clone(), DEFAULT_TREE_DEPTH),
            hasher,
        }
    }

    /// Compute the combined root hash of all 6 trees
    ///
    /// Structure:
    /// ```text
    ///                  ROOT
    ///                /      \
    ///            L1            R1
    ///           /  \          /  \
    ///         L2    T3      L3    T6
    ///        /  \          /  \
    ///       T1  T2        T4  T5
    /// ```
    /// Where T1=Account, T2=AccountOrder, T3=OrderBook, T4=Position, T5=Pool, T6=System
    pub fn root(&self) -> Hash {
        let t1 = self.account_tree.root();
        let t2 = self.account_order_tree.root();
        let t3 = self.orderbook_tree.root();
        let t4 = self.position_tree.root();
        let t5 = self.pool_tree.root();
        let t6 = self.system_tree.root();

        // Left subtree: hash(hash(T1, T2), T3)
        let l2 = self.hasher.hash_pair(&t1, &t2);
        let l1 = self.hasher.hash_pair(&l2, &t3);

        // Right subtree: hash(hash(T4, T5), T6)
        let l3 = self.hasher.hash_pair(&t4, &t5);
        let r1 = self.hasher.hash_pair(&l3, &t6);

        // Root: hash(L1, R1)
        self.hasher.hash_pair(&l1, &r1)
    }

    /// Get individual tree roots
    pub fn tree_roots(&self) -> [Hash; 6] {
        [
            self.account_tree.root(),
            self.account_order_tree.root(),
            self.orderbook_tree.root(),
            self.position_tree.root(),
            self.pool_tree.root(),
            self.system_tree.root(),
        ]
    }

    /// Get a specific tree by ID
    pub fn get_tree(&self, tree_id: TreeId) -> &SparseMerkleTree<H> {
        match tree_id {
            TreeId::Account => &self.account_tree,
            TreeId::AccountOrder => &self.account_order_tree,
            TreeId::OrderBook => &self.orderbook_tree,
            TreeId::Position => &self.position_tree,
            TreeId::Pool => &self.pool_tree,
            TreeId::System => &self.system_tree,
        }
    }

    /// Get a mutable reference to a specific tree
    pub fn get_tree_mut(&mut self, tree_id: TreeId) -> &mut SparseMerkleTree<H> {
        match tree_id {
            TreeId::Account => &mut self.account_tree,
            TreeId::AccountOrder => &mut self.account_order_tree,
            TreeId::OrderBook => &mut self.orderbook_tree,
            TreeId::Position => &mut self.position_tree,
            TreeId::Pool => &mut self.pool_tree,
            TreeId::System => &mut self.system_tree,
        }
    }

    // ========== Account Tree Operations ==========

    /// Compute key for account in Account Tree
    pub fn account_key(&self, account_id: AccountId) -> Hash {
        self.hasher.hash(&account_id.to_le_bytes())
    }

    /// Get account from tree
    pub fn get_account(&self, account_id: AccountId) -> Option<Account> {
        let key = self.account_key(account_id);
        self.account_tree.get(&key).and_then(|hash| {
            // In a real implementation, we'd store serialized data
            // For now, we just check existence
            if *hash != ZERO_HASH {
                // Would deserialize from storage
                None
            } else {
                None
            }
        })
    }

    /// Insert account into tree
    pub fn insert_account(&mut self, account: &Account) {
        let key = self.account_key(account.id);
        let value = self.hasher.hash(&bincode::serialize(account).unwrap_or_default());
        self.account_tree.insert(key, value);
    }

    // ========== Order Book Tree Operations ==========

    /// Compute key for order in Order Book Tree
    /// Uses price-nonce encoding for price-time priority
    pub fn orderbook_key(&self, order: &Order) -> Hash {
        let path = order.orderbook_path();
        self.hasher.hash(&path)
    }

    /// Insert order into Order Book Tree
    pub fn insert_order(&mut self, order: &Order) {
        let key = self.orderbook_key(order);
        let value = self.hasher.hash(&bincode::serialize(order).unwrap_or_default());
        self.orderbook_tree.insert(key, value);
    }

    /// Remove order from Order Book Tree
    pub fn remove_order(&mut self, order: &Order) {
        let key = self.orderbook_key(order);
        self.orderbook_tree.remove(&key);
    }

    // ========== Position Tree Operations ==========

    /// Compute key for position
    pub fn position_key(&self, account_id: AccountId, market_id: MarketId) -> Hash {
        let mut data = account_id.to_le_bytes().to_vec();
        data.extend_from_slice(&market_id.to_le_bytes());
        self.hasher.hash(&data)
    }

    /// Insert position into tree
    pub fn insert_position(&mut self, position: &Position) {
        let key = self.position_key(position.account_id, position.market_id);
        let value = self.hasher.hash(&bincode::serialize(position).unwrap_or_default());
        self.position_tree.insert(key, value);
    }

    /// Remove position from tree
    pub fn remove_position(&mut self, account_id: AccountId, market_id: MarketId) {
        let key = self.position_key(account_id, market_id);
        self.position_tree.remove(&key);
    }

    // ========== Proof Generation ==========

    /// Generate proof for account
    pub fn prove_account(&self, account_id: AccountId) -> MerkleProof {
        let key = self.account_key(account_id);
        self.account_tree.prove(&key)
    }

    /// Generate proof for order
    pub fn prove_order(&self, order: &Order) -> MerkleProof {
        let key = self.orderbook_key(order);
        self.orderbook_tree.prove(&key)
    }

    /// Generate proof for position
    pub fn prove_position(&self, account_id: AccountId, market_id: MarketId) -> MerkleProof {
        let key = self.position_key(account_id, market_id);
        self.position_tree.prove(&key)
    }

    /// Generate combined witness for a state transition
    pub fn generate_witness(
        &self,
        account_ids: &[AccountId],
        orders: &[Order],
        position_keys: &[(AccountId, MarketId)],
    ) -> HypertreeWitness {
        let pre_root = self.root();
        let mut proofs = Vec::new();

        // Account proofs
        for &id in account_ids {
            proofs.push((TreeId::Account, self.prove_account(id)));
        }

        // Order proofs
        for order in orders {
            proofs.push((TreeId::OrderBook, self.prove_order(order)));
        }

        // Position proofs
        for &(account_id, market_id) in position_keys {
            proofs.push((TreeId::Position, self.prove_position(account_id, market_id)));
        }

        HypertreeWitness {
            pre_root,
            post_root: pre_root, // Will be updated after state changes
            proofs,
        }
    }
}

impl Default for Hypertree<Sha256Hasher> {
    fn default() -> Self {
        Self::new(Sha256Hasher::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_hypertree() {
        let tree: Hypertree<Sha256Hasher> = Hypertree::default();
        let root = tree.root();

        // Root should be deterministic for empty tree
        let tree2: Hypertree<Sha256Hasher> = Hypertree::default();
        assert_eq!(root, tree2.root());
    }

    #[test]
    fn test_root_changes_on_modification() {
        let mut tree: Hypertree<Sha256Hasher> = Hypertree::default();
        let initial_root = tree.root();

        // Insert an account
        let account = Account::new(1, [0u8; 32], 0);
        tree.insert_account(&account);

        assert_ne!(initial_root, tree.root());
    }

    #[test]
    fn test_tree_roots() {
        let tree: Hypertree<Sha256Hasher> = Hypertree::default();
        let roots = tree.tree_roots();

        assert_eq!(roots.len(), 6);
        // All empty trees should have same root
        assert_eq!(roots[0], roots[1]);
    }

    #[test]
    fn test_order_key_price_priority() {
        let tree: Hypertree<Sha256Hasher> = Hypertree::default();

        // Two bids at different prices
        let high_bid = Order::new_limit(1, 1, 0, Side::Bid, 50000, 1000, 0, 0);
        let low_bid = Order::new_limit(2, 1, 0, Side::Bid, 49000, 1000, 0, 0);

        let high_key = tree.orderbook_key(&high_bid);
        let low_key = tree.orderbook_key(&low_bid);

        // Keys should be different
        assert_ne!(high_key, low_key);

        // Two asks at different prices
        let low_ask = Order::new_limit(3, 1, 0, Side::Ask, 51000, 1000, 0, 0);
        let high_ask = Order::new_limit(4, 1, 0, Side::Ask, 52000, 1000, 0, 0);

        let low_ask_key = tree.orderbook_key(&low_ask);
        let high_ask_key = tree.orderbook_key(&high_ask);

        assert_ne!(low_ask_key, high_ask_key);
    }

    #[test]
    fn test_position_key_uniqueness() {
        let tree: Hypertree<Sha256Hasher> = Hypertree::default();

        // Same account, different markets
        let key1 = tree.position_key(1, 0);
        let key2 = tree.position_key(1, 1);
        assert_ne!(key1, key2);

        // Different accounts, same market
        let key3 = tree.position_key(2, 0);
        assert_ne!(key1, key3);
    }
}
