//! Position types for the perpetual DEX

use serde::{Deserialize, Serialize};
use super::{AccountId, MarketId, Price, Quantity, Timestamp};

/// Position side: long or short
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PositionSide {
    /// No position
    #[default]
    None,
    /// Long position (profit when price goes up)
    Long,
    /// Short position (profit when price goes down)
    Short,
}

impl PositionSide {
    /// Check if position is open (has a side)
    pub fn is_open(&self) -> bool {
        !matches!(self, PositionSide::None)
    }

    /// Get the opposite side
    pub fn opposite(&self) -> Self {
        match self {
            PositionSide::None => PositionSide::None,
            PositionSide::Long => PositionSide::Short,
            PositionSide::Short => PositionSide::Long,
        }
    }
}

/// An open position in a market
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Position {
    /// Account that holds this position
    pub account_id: AccountId,
    /// Market this position is in
    pub market_id: MarketId,
    /// Long or short
    pub side: PositionSide,
    /// Position size (in base asset, 18 decimals)
    pub size: Quantity,
    /// Average entry price (8 decimals)
    pub entry_price: Price,
    /// Margin allocated to this position (quote asset)
    pub margin: u128,
    /// Leverage used (1-50)
    pub leverage: u8,
    /// Liquidation price (8 decimals)
    pub liquidation_price: Price,
    /// Timestamp of last update
    pub last_update: Timestamp,
    /// Accumulated funding payments (can be negative)
    pub accumulated_funding: i128,
    /// Timestamp of last funding payment
    pub last_funding_timestamp: Timestamp,
}

impl Position {
    /// Create a new position
    pub fn new(
        account_id: AccountId,
        market_id: MarketId,
        side: PositionSide,
        size: Quantity,
        entry_price: Price,
        margin: u128,
        leverage: u8,
        timestamp: Timestamp,
    ) -> Self {
        let liquidation_price = Self::calculate_liquidation_price(
            side,
            entry_price,
            leverage,
            100, // 1% maintenance margin
        );

        Self {
            account_id,
            market_id,
            side,
            size,
            entry_price,
            margin,
            leverage,
            liquidation_price,
            last_update: timestamp,
            accumulated_funding: 0,
            last_funding_timestamp: timestamp,
        }
    }

    /// Check if position is open
    pub fn is_open(&self) -> bool {
        self.side.is_open() && self.size > 0
    }

    /// Calculate unrealized PnL at a given mark price
    /// Returns (is_profit, pnl_amount)
    pub fn unrealized_pnl(&self, mark_price: Price) -> (bool, u128) {
        if !self.is_open() {
            return (true, 0);
        }

        let entry = self.entry_price as i128;
        let mark = mark_price as i128;
        let size = self.size as i128;

        // PnL = size * (mark_price - entry_price) for long
        // PnL = size * (entry_price - mark_price) for short
        let pnl_raw = match self.side {
            PositionSide::Long => (mark - entry) * size / 100_000_000, // Adjust for price decimals
            PositionSide::Short => (entry - mark) * size / 100_000_000,
            PositionSide::None => 0,
        };

        // Divide by 10^18 to normalize from quantity decimals
        let pnl = pnl_raw / 1_000_000_000_000_000_000;

        if pnl >= 0 {
            (true, pnl as u128)
        } else {
            (false, (-pnl) as u128)
        }
    }

    /// Calculate margin ratio (account equity / position value)
    /// Returns ratio in basis points (10000 = 100%)
    pub fn margin_ratio(&self, mark_price: Price) -> u16 {
        if !self.is_open() {
            return 10000;
        }

        let (is_profit, pnl) = self.unrealized_pnl(mark_price);
        let equity = if is_profit {
            self.margin + pnl
        } else {
            self.margin.saturating_sub(pnl)
        };

        // Position value = size * mark_price / 10^18
        let position_value = (self.size as u128) * (mark_price as u128) / 1_000_000_000_000_000_000;

        if position_value == 0 {
            return 10000;
        }

        ((equity * 10000) / position_value) as u16
    }

    /// Check if position can be liquidated at given mark price
    pub fn is_liquidatable(&self, mark_price: Price, maintenance_margin_bps: u16) -> bool {
        self.margin_ratio(mark_price) < maintenance_margin_bps
    }

    /// Calculate liquidation price
    fn calculate_liquidation_price(
        side: PositionSide,
        entry_price: Price,
        leverage: u8,
        maintenance_margin_bps: u16,
    ) -> Price {
        // Simplified liquidation price calculation
        // For long: liq_price = entry_price * (1 - 1/leverage + maintenance_margin)
        // For short: liq_price = entry_price * (1 + 1/leverage - maintenance_margin)

        let entry = entry_price as u128;
        let lev = leverage as u128;
        let mm = maintenance_margin_bps as u128;

        match side {
            PositionSide::Long => {
                // liq = entry * (1 - (1/lev - mm/10000))
                // liq = entry * (lev*10000 - 10000 + mm*lev) / (lev * 10000)
                let numerator = entry * (lev * 10000 - 10000 + mm * lev);
                let denominator = lev * 10000;
                (numerator / denominator) as Price
            }
            PositionSide::Short => {
                // liq = entry * (1 + 1/lev - mm/10000)
                let numerator = entry * (lev * 10000 + 10000 - mm * lev);
                let denominator = lev * 10000;
                (numerator / denominator) as Price
            }
            PositionSide::None => 0,
        }
    }

    /// Increase position size (add to existing position)
    pub fn increase(
        &mut self,
        additional_size: Quantity,
        price: Price,
        additional_margin: u128,
        timestamp: Timestamp,
    ) {
        // Calculate new weighted average entry price
        let old_value = (self.size as u128) * (self.entry_price as u128);
        let new_value = (additional_size as u128) * (price as u128);
        let total_size = self.size + additional_size;

        if total_size > 0 {
            self.entry_price = ((old_value + new_value) / (total_size as u128)) as Price;
        }

        self.size = total_size;
        self.margin += additional_margin;
        self.last_update = timestamp;

        // Recalculate liquidation price
        self.liquidation_price = Self::calculate_liquidation_price(
            self.side,
            self.entry_price,
            self.leverage,
            100,
        );
    }

    /// Decrease position size (reduce or close position)
    /// Returns realized PnL (positive for profit, negative for loss)
    pub fn decrease(
        &mut self,
        reduce_size: Quantity,
        price: Price,
        timestamp: Timestamp,
    ) -> i128 {
        let reduce_size = reduce_size.min(self.size);

        // Calculate realized PnL for the portion being closed
        let entry = self.entry_price as i128;
        let exit = price as i128;
        let size = reduce_size as i128;

        let pnl = match self.side {
            PositionSide::Long => (exit - entry) * size / 100_000_000 / 1_000_000_000_000_000_000,
            PositionSide::Short => (entry - exit) * size / 100_000_000 / 1_000_000_000_000_000_000,
            PositionSide::None => 0,
        };

        // Reduce margin proportionally
        let margin_reduction = (self.margin as u128) * (reduce_size as u128) / (self.size as u128);
        self.margin = self.margin.saturating_sub(margin_reduction as u128);

        self.size -= reduce_size;
        self.last_update = timestamp;

        // Close position if size is zero
        if self.size == 0 {
            self.side = PositionSide::None;
            self.entry_price = 0;
            self.liquidation_price = 0;
            self.margin = 0;
        }

        pnl
    }

    /// Flip position side (close current and open opposite)
    pub fn flip(
        &mut self,
        new_side: PositionSide,
        new_size: Quantity,
        price: Price,
        margin: u128,
        leverage: u8,
        timestamp: Timestamp,
    ) -> i128 {
        // Close existing position
        let pnl = self.decrease(self.size, price, timestamp);

        // Open new position
        self.side = new_side;
        self.size = new_size;
        self.entry_price = price;
        self.margin = margin;
        self.leverage = leverage;
        self.last_update = timestamp;
        self.liquidation_price = Self::calculate_liquidation_price(new_side, price, leverage, 100);

        pnl
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_unrealized_pnl() {
        // Long position: 1 BTC at $50,000
        let position = Position::new(
            1,
            0,
            PositionSide::Long,
            1_000_000_000_000_000_000, // 1.0 BTC
            50000_00000000,            // $50,000
            1000_00000000,             // $1,000 margin
            10,
            0,
        );

        // Price goes up to $51,000 -> profit
        let (is_profit, pnl) = position.unrealized_pnl(51000_00000000);
        assert!(is_profit);
        assert!(pnl > 0);

        // Price goes down to $49,000 -> loss
        let (is_profit, pnl) = position.unrealized_pnl(49000_00000000);
        assert!(!is_profit);
        assert!(pnl > 0);
    }

    #[test]
    fn test_position_side_opposite() {
        assert_eq!(PositionSide::Long.opposite(), PositionSide::Short);
        assert_eq!(PositionSide::Short.opposite(), PositionSide::Long);
        assert_eq!(PositionSide::None.opposite(), PositionSide::None);
    }

    #[test]
    fn test_position_decrease_close() {
        let mut position = Position::new(
            1, 0,
            PositionSide::Long,
            1_000_000_000_000_000_000,
            50000_00000000,
            1000_00000000,
            10,
            0,
        );

        // Close entire position
        position.decrease(1_000_000_000_000_000_000, 51000_00000000, 100);

        assert!(!position.is_open());
        assert_eq!(position.size, 0);
        assert_eq!(position.side, PositionSide::None);
    }
}
