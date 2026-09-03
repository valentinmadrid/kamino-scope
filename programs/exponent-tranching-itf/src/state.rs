use anchor_lang::prelude::*;
use anchor_lang::Discriminator;

/// Converts Exponent's 12-decimal fixed-point representation to Scope's 18-decimal WAD.
/// Source: https://github.com/exponent-finance/exponent-core/blob/f250d0bf4ebbaa12d5a69015c68d212fb1643f60/libraries/precise_number/src/lib.rs#L37-L49
pub const EXPONENT_NUMBER_SCALE_TO_WAD: u64 = 1_000_000;

// Skip fields unrelated to the CPI account list to stay within the SBF stack limit.
const MARKET_FIELDS_BEFORE_ROLES_SIZE: usize = 1017;

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

/// Market prefix through return_model_storage; later fields are parsed selectively
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
}

pub struct MarketCpiConfig {
    pub address_lookup_table: Pubkey,
    pub sy_program: Pubkey,
    pub return_model_storage: Pubkey,
    pub get_sy_state: Vec<CpiInterfaceContext>,
}

#[derive(AnchorDeserialize)]
pub struct CpiInterfaceContext {
    pub alt_index: u8,
    pub is_signer: bool,
    pub is_writable: bool,
}

#[derive(AnchorDeserialize)]
struct MarketCpiTail {
    _admin: Vec<Pubkey>,
    _sentinel: Vec<Pubkey>,
    get_sy_state: Vec<CpiInterfaceContext>,
}

impl ExponentTranchingMarket {
    pub fn read_cpi_config(mut data: &[u8]) -> Option<MarketCpiConfig> {
        if data.get(..8)? != Self::discriminator() {
            return None;
        }

        data = data.get(8..)?;
        let market = Self::deserialize(&mut data).ok()?;
        data = data.get(MARKET_FIELDS_BEFORE_ROLES_SIZE..)?;
        let tail = MarketCpiTail::deserialize(&mut data).ok()?;

        Some(MarketCpiConfig {
            address_lookup_table: market.address_lookup_table,
            sy_program: market.sy_program,
            return_model_storage: market.return_model_storage,
            get_sy_state: tail.get_sy_state,
        })
    }
}
