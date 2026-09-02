use anchor_lang::prelude::*;

/// Source: https://github.com/exponent-finance/exponent-core/blob/f250d0bf4ebbaa12d5a69015c68d212fb1643f60/libraries/precise_number/src/lib.rs#L37-L49
pub const EXPONENT_NUMBER_DENOMINATOR: u128 = 1_000_000_000_000;
/// Converts Exponent's 12-decimal fixed-point representation to Scope's 18-decimal WAD.
pub const EXPONENT_NUMBER_SCALE_TO_WAD: u64 = 1_000_000;

const LAST_UPDATED_SLOT_SIZE: usize = 8;
const UTILIZATION_GUIDED_CURVE_PARAMS_SIZE: usize = (4 * ExponentNumber::SIZEOF) + 8;
const RETURN_MODEL_RESERVED_PADDING_SIZE: usize =
    1 + UTILIZATION_GUIDED_CURVE_PARAMS_SIZE - LAST_UPDATED_SLOT_SIZE;

#[derive(Clone, Copy, Debug, Eq, PartialEq, AnchorDeserialize, AnchorSerialize)]
pub struct ExponentNumber(pub [u64; 4]);

impl ExponentNumber {
    const SIZEOF: usize = 32;

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

#[derive(AnchorDeserialize, AnchorSerialize)]
pub struct UpdateMarketReturnData {
    pub market: Pubkey,
    pub sy_exchange_rate: ExponentNumber,
    pub senior_raw_nav: ExponentNumber,
    pub junior_raw_nav: ExponentNumber,
    pub senior_effective_nav: ExponentNumber,
    pub junior_effective_nav: ExponentNumber,
    pub senior_loss: ExponentNumber,
    pub junior_loss: ExponentNumber,
    pub senior_premium: ExponentNumber,
    pub junior_premium: ExponentNumber,
    pub utilization: ExponentNumber,
    pub senior_lp_price_net_asset: ExponentNumber,
    pub junior_lp_price_net_asset: ExponentNumber,
    pub timestamp: i64,
}

/// Source: https://github.com/exponent-finance/exponent-monorepo/blob/fb36c935200a4ef0348d7b8235a3517dfc54fd90/solana/programs/exponent_tranching/src/state/exponent_tranching_market/exponent_tranching_market.rs
#[account]
pub struct ExponentTranchingMarket {
    pub address_lookup_table: Pubkey,
    pub sy_mint: Pubkey,
    pub sy_program: Pubkey,
    pub token_sy_escrow: Pubkey,
    pub mint_lp_senior: Pubkey,
    pub mint_lp_junior: Pubkey,
    pub self_address: Pubkey,
    pub return_model_storage: Pubkey,
    pub signer_bump: [u8; 1],
    pub return_model_storage_bump: [u8; 1],
    pub seed_id: [u8; 8],
    pub status_flags: u8,
    pub market_state: TranchingMarketState,
    pub financials: TranchingMarketFinancials,
    pub tranche_supply_state: TrancheSupplyState,
    pub tranche_asset_state: TrancheAssetState,
    pub risk_config: TranchingRiskConfig,
    pub protocol_fee_config: TranchingProtocolFeeConfig,
    pub last_updated_slot: u64,
    pub reserved_padding: [u8; RETURN_MODEL_RESERVED_PADDING_SIZE],
    pub roles: TranchingMarketRoles,
    pub sy_cpi_accounts: CpiAccounts,
}

#[derive(Clone, Copy, AnchorDeserialize, AnchorSerialize)]
pub enum TranchingMarketState {
    Uninitialized,
    Active,
    FixedTermRecovery,
}

#[derive(Clone, Copy, AnchorDeserialize, AnchorSerialize)]
pub struct TranchingMarketFinancials {
    pub sr_raw_net_asset: ExponentNumber,
    pub jr_raw_net_asset: ExponentNumber,
    pub sr_effective_net_asset: ExponentNumber,
    pub jr_effective_net_asset: ExponentNumber,
    pub sr_impermanent_loss: ExponentNumber,
    pub jr_impermanent_loss: ExponentNumber,
    pub utilization: ExponentNumber,
    pub current_junior_return_share: ExponentNumber,
    pub tw_junior_return_share_accrued: ExponentNumber,
    pub last_sync_ts: i64,
    pub last_distribution_ts: i64,
    pub fixed_term_end_ts: i64,
}

#[derive(Clone, Copy, AnchorDeserialize, AnchorSerialize)]
pub struct TrancheSupplyState {
    pub total_senior_lp_supply: u64,
    pub total_junior_lp_supply: u64,
    pub max_senior_lp_supply: u64,
    pub max_junior_lp_supply: u64,
    pub pending_senior_protocol_fee_lp_shares: u64,
    pub pending_junior_protocol_fee_lp_shares: u64,
    pub pending_senior_deposit_protocol_fee_lp_shares: u64,
    pub pending_junior_deposit_protocol_fee_lp_shares: u64,
    pub pending_senior_withdraw_protocol_fee_lp_shares: u64,
    pub pending_junior_withdraw_protocol_fee_lp_shares: u64,
}

#[derive(Clone, Copy, AnchorDeserialize, AnchorSerialize)]
pub struct TrancheAssetState {
    pub senior_sy_amount: u64,
    pub junior_sy_amount: u64,
}

#[derive(Clone, Copy, AnchorDeserialize, AnchorSerialize)]
pub struct TranchingRiskConfig {
    pub min_coverage: ExponentNumber,
    pub beta: ExponentNumber,
    pub liquidation_utilization: ExponentNumber,
    pub fixed_term_duration_sec: u32,
    pub min_deposit_amount: u64,
    pub sr_self_liquidation_bonus: ExponentNumber,
    pub sr_net_asset_dust_tolerance: ExponentNumber,
    pub jr_net_asset_dust_tolerance: ExponentNumber,
}

#[derive(Clone, Copy, AnchorDeserialize, AnchorSerialize)]
pub struct TranchingProtocolFeeConfig {
    pub protocol_fee_recipient: Pubkey,
    pub sr_protocol_fee: ExponentNumber,
    pub jr_protocol_fee: ExponentNumber,
    pub junior_return_protocol_fee: ExponentNumber,
    pub senior_deposit_protocol_fee: ExponentNumber,
    pub junior_deposit_protocol_fee: ExponentNumber,
    pub senior_withdraw_protocol_fee: ExponentNumber,
    pub junior_withdraw_protocol_fee: ExponentNumber,
}

#[derive(Clone, AnchorDeserialize, AnchorSerialize)]
pub struct TranchingMarketRoles {
    pub admin: Vec<Pubkey>,
    pub sentinel: Vec<Pubkey>,
}

#[derive(Clone, AnchorDeserialize, AnchorSerialize)]
pub struct CpiAccounts {
    pub get_sy_state: Vec<CpiInterfaceContext>,
}

#[derive(Clone, AnchorDeserialize, AnchorSerialize)]
pub struct CpiInterfaceContext {
    pub alt_index: u8,
    pub is_signer: bool,
    pub is_writable: bool,
}
