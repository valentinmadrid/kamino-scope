use anchor_lang::prelude::*;

use exponent_core_itf::ExponentNumber;

declare_id!("XP1BRLn8eCYSygrd8er5P4GKdzqKbC3DLoSsS5UYVZy");

// Generic Standard uses a one-byte instruction tag, which Anchor 0.28 cannot derive.
pub const GET_STATE_DISCRIMINATOR: [u8; 1] = [7];

pub mod accounts {
    pub use super::__client_accounts_get_sy_state::GetSyState;
}

#[derive(Accounts)]
pub struct GetSyState<'info> {
    /// CHECK: interface only.
    #[account(mut)]
    pub meta: UncheckedAccount<'info>,
    /// CHECK: interface only.
    pub mint_sy: UncheckedAccount<'info>,
    /// CHECK: interface only.
    pub token_sy_escrow: UncheckedAccount<'info>,
}

/// Source: https://github.com/exponent-finance/exponent-core/blob/f250d0bf4ebbaa12d5a69015c68d212fb1643f60/libraries/sy_common/src/lib.rs
#[derive(AnchorDeserialize, AnchorSerialize)]
pub struct SyState {
    pub exchange_rate: ExponentNumber,
    pub emission_indexes: Vec<ExponentNumber>,
}

/// Only the mint binding is needed; get_state validates the rest of the SY account.
#[account]
pub struct SyMeta {
    pub mint_sy: Pubkey,
}
