#![allow(clippy::result_large_err)]

pub mod state;

use anchor_lang::prelude::*;
pub use state::*;

declare_id!("XPTrnchoawiUc9iYJrpfchS8vgr8Y5X2QGBdHPXukty");

#[program]
pub mod exponent_tranching {
    use super::*;

    #[allow(unused_variables)]
    pub fn update_market(ctx: Context<UpdateMarket>) -> Result<UpdateMarketReturnData> {
        unimplemented!("exponent-tranching-itf is just an interface")
    }
}

#[derive(Accounts)]
pub struct UpdateMarket<'info> {
    /// CHECK: interface only.
    #[account(mut)]
    pub market: UncheckedAccount<'info>,
    /// CHECK: interface only.
    #[account(mut)]
    pub return_model_storage: UncheckedAccount<'info>,
    /// CHECK: interface only.
    pub address_lookup_table: UncheckedAccount<'info>,
    /// CHECK: interface only.
    pub sy_program: UncheckedAccount<'info>,
    /// CHECK: interface only.
    pub event_authority: UncheckedAccount<'info>,
    /// CHECK: interface only.
    pub program: UncheckedAccount<'info>,
}
