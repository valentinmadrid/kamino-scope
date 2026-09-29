use anchor_lang::prelude::*;
use decimal_wad::common::uint::U192;

/// Source: https://github.com/exponent-finance/exponent-core/blob/f250d0bf4ebbaa12d5a69015c68d212fb1643f60/libraries/precise_number/src/lib.rs#L37-L49
pub const EXPONENT_NUMBER_SCALE_TO_WAD: u64 = 1_000_000;
pub const EXPONENT_NUMBER_DENOMINATOR: u128 = 1_000_000_000_000;

#[derive(Clone, Copy, AnchorDeserialize, AnchorSerialize)]
pub struct ExponentNumber(pub [u64; 4]);

impl ExponentNumber {
    pub fn raw_u128(self) -> Option<u128> {
        let [low, high, upper_low, upper_high] = self.0;
        if upper_low != 0 || upper_high != 0 {
            return None;
        }
        Some(u128::from(low) | (u128::from(high) << 64))
    }

    pub fn is_zero(self) -> bool {
        self.0 == [0; 4]
    }
}

#[derive(Clone, AnchorDeserialize, AnchorSerialize)]
pub struct CpiInterfaceContext {
    pub alt_index: u8,
    pub is_signer: bool,
    pub is_writable: bool,
}

/// Vault prefix through the get_sy_state account list; the remaining CPI lists are unused.
/// Source: https://github.com/exponent-finance/exponent-core/blob/f250d0bf4ebbaa12d5a69015c68d212fb1643f60/programs/exponent_core/src/state/vault.rs
#[account]
pub struct Vault {
    pub sy_program: Pubkey,
    pub mint_sy: Pubkey,
    pub mint_yt: Pubkey,
    pub mint_pt: Pubkey,
    pub escrow_yt: Pubkey,
    pub escrow_sy: Pubkey,
    pub yield_position: Pubkey,
    pub address_lookup_table: Pubkey,
    pub start_ts: u32,
    pub duration: u32,
    pub signer_seed: Pubkey,
    pub authority: Pubkey,
    pub signer_bump: [u8; 1],
    pub last_seen_sy_exchange_rate: ExponentNumber,
    pub all_time_high_sy_exchange_rate: ExponentNumber,
    pub final_sy_exchange_rate: ExponentNumber,
    pub total_sy_in_escrow: u64,
    pub sy_for_pt: u64,
    pub pt_supply: u64,
    pub treasury_sy: u64,
    pub uncollected_sy: u64,
    pub treasury_sy_token_account: Pubkey,
    pub interest_bps_fee: u16,
    pub min_op_size_strip: u64,
    pub min_op_size_merge: u64,
    pub status: u8,
    pub emissions: Vec<EmissionInfo>,
    pub cpi_accounts: CpiAccounts,
}

impl Vault {
    /// Copies Exponent core's PT backing and redemption formulas:
    /// - https://github.com/exponent-finance/exponent-core/blob/f250d0bf4ebbaa12d5a69015c68d212fb1643f60/programs/exponent_core/src/state/vault.rs#L569-L580
    /// - https://github.com/exponent-finance/exponent-core/blob/f250d0bf4ebbaa12d5a69015c68d212fb1643f60/programs/exponent_core/src/state/vault.rs#L139-L142
    /// TODO: update the pinned revision when the negative-yield logic is published.
    ///
    /// Prices PT using the current SY exchange rate and the vault's current redemption state.
    /// Calculates SY backing with max(live rate, stored all-time high), then values it at the live rate.
    /// Scope does not persist an observed high. If the rate later falls, the next quote uses the vault's
    /// stored watermark, not the previous Scope observation: a peak never recorded by the vault does
    /// not increase PT's redemption watermark. Each quote therefore reflects the current PT backing,
    /// including any loss against that watermark, with the underlying value capped at par.
    /// Applies the supplied linear discount and returns the result with 12 decimal places.
    pub fn apply_underlying_discount(
        &self,
        discounted_value: u64,
        discount_scale: u64,
        exchange_rate: u128,
    ) -> Option<u64> {
        let high_water = self.all_time_high_sy_exchange_rate.raw_u128()?;
        if high_water == 0 || self.pt_supply == 0 {
            return None;
        }

        let denominator = EXPONENT_NUMBER_DENOMINATOR;
        let active_sy = self
            .total_sy_in_escrow
            .checked_sub(self.treasury_sy)?
            .checked_sub(self.uncollected_sy)?;

        let backing = u128::from(self.pt_supply)
            .checked_mul(denominator)?
            .checked_div(high_water.max(exchange_rate))?
            .min(u128::from(active_sy));
        let redemption_rate = backing
            .checked_mul(denominator)?
            .checked_div(u128::from(self.pt_supply))?;

        let value = U192::from(redemption_rate)
            .checked_mul(U192::from(exchange_rate))?
            .checked_div(U192::from(denominator))?
            .checked_mul(U192::from(discounted_value))?
            .checked_div(U192::from(discount_scale))?;

        value.try_into().ok()
    }
}

#[derive(Clone, AnchorDeserialize, AnchorSerialize)]
pub struct EmissionInfo {
    pub token_account: Pubkey,
    pub initial_index: ExponentNumber,
    pub last_seen_index: ExponentNumber,
    pub final_index: ExponentNumber,
    pub treasury_token_account: Pubkey,
    pub fee_bps: u16,
    pub treasury_emission: u64,
}

#[derive(Clone, AnchorDeserialize, AnchorSerialize)]
pub struct CpiAccounts {
    pub get_sy_state: Vec<CpiInterfaceContext>,
}
