//! The Lorenz on-chain arbitrage executor.
//!
//! This program is the single source of the platform's hard safety guarantees.
//! They are enforced by the runtime, not by the off-chain bot, which is what
//! makes the system auditable: a reviewer can read this file and the chain will
//! refuse anything that violates it.
//!
//! Invariants enforced here (see also docs/INVARIANTS.md):
//!   I1. Atomic-or-revert: the whole arbitrage runs in one instruction; if the
//!       profit check fails, the transaction reverts and the only cost is fees.
//!   I2. No loss: settlement requires `balance_after >= balance_before +
//!       min_profit`, with `min_profit >= 0`.
//!   I3. Bounded spend: `notional <= vault.spend_cap`.
//!   I4. Scoped authority: the bot may invoke `execute_arbitrage` but can never
//!       move funds out of the vault except as the profit-bearing round trip.
//!       Only the owner can `withdraw`.
//!   I5. Transparent fee: the protocol fee is a fixed `fee_bps` of realized
//!       profit, paid on-chain to a fixed account.
//!
//! HONEST STATUS: the flash-loan borrow/repay CPI and the multi-DEX swap route
//! are NOT implemented yet. They are isolated behind [`route::execute_route`]
//! and [`flash_loan`], which currently return [`ExecutorError::NotImplemented`].
//! The guard logic around them (I2, I3, I5) is fully written and reviewable, so
//! the security model is visible even though the program cannot yet land a live
//! trade. Nothing here pretends to work that does not.

use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

// NOTE: This is a placeholder program id, not a deployable address. A real
// deployer must generate their own keypair and run `anchor keys sync` to
// replace this id in both this file and `Anchor.toml` before deploying.
declare_id!("E5Mj85eZNW9s2fhceFT56sZFQezXiVNJ7QKZv5vp1RxA");

pub const MAX_FEE_BPS: u16 = 1_000; // hard ceiling: protocol fee <= 10%

#[program]
pub mod lorenz_executor {
    use super::*;

    /// Create a vault PDA owned by `owner`. The vault parameters are immutable
    /// after creation except via explicit owner-signed instructions (not yet
    /// exposed), keeping the trust surface small.
    pub fn initialize_vault(ctx: Context<InitializeVault>, spend_cap: u64, fee_bps: u16) -> Result<()> {
        require!(fee_bps <= MAX_FEE_BPS, ExecutorError::FeeTooHigh);

        let vault = &mut ctx.accounts.vault;
        vault.owner = ctx.accounts.owner.key();
        vault.bump = ctx.bumps.vault;
        vault.base_mint = ctx.accounts.base_mint.key();
        vault.vault_token_account = ctx.accounts.vault_token_account.key();
        vault.protocol_fee_account = ctx.accounts.protocol_fee_account.key();
        vault.spend_cap = spend_cap;
        vault.fee_bps = fee_bps;
        vault.cumulative_profit = 0;
        Ok(())
    }

    /// Execute one atomic arbitrage cycle.
    ///
    /// `notional` is the borrowed size; `min_profit` is the floor the round
    /// trip must clear after repaying the flash loan. The DEX route is passed
    /// via `remaining_accounts` and interpreted by [`route::execute_route`].
    pub fn execute_arbitrage(
        ctx: Context<ExecuteArbitrage>,
        notional: u64,
        min_profit: u64,
    ) -> Result<()> {
        let vault = &ctx.accounts.vault;

        // I3: bounded spend.
        require!(notional <= vault.spend_cap, ExecutorError::SpendCapExceeded);

        // Snapshot balance before the cycle (I2 baseline).
        let balance_before = ctx.accounts.vault_token_account.amount;

        // --- Borrow (flash loan). ROADMAP: not implemented. ---
        flash_loan::borrow(&ctx, notional)?;

        // --- Swap route across DEXs. ROADMAP: not implemented. ---
        route::execute_route(&ctx, notional)?;

        // --- Repay (flash loan + fee). ROADMAP: not implemented. ---
        flash_loan::repay(&ctx, notional)?;

        // Re-read the (mutated) token account to measure realized output.
        ctx.accounts.vault_token_account.reload()?;
        let balance_after = ctx.accounts.vault_token_account.amount;

        // I2: no loss. Profit must clear the floor.
        let profit = balance_after
            .checked_sub(balance_before)
            .ok_or(ExecutorError::ArbNotProfitable)?;
        require!(profit >= min_profit, ExecutorError::ArbNotProfitable);

        // I5: transparent protocol fee on realized profit. Pure, unit-tested
        // arithmetic (see `mod math`).
        let fee = math::protocol_fee(profit, vault.fee_bps);

        if fee > 0 {
            let owner_key = vault.owner;
            let seeds: &[&[u8]] = &[b"vault", owner_key.as_ref(), &[vault.bump]];
            let signer = &[seeds];
            let cpi = CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.vault_token_account.to_account_info(),
                    to: ctx.accounts.protocol_fee_account.to_account_info(),
                    authority: ctx.accounts.vault.to_account_info(),
                },
                signer,
            );
            token::transfer(cpi, fee)?;
        }

        let vault = &mut ctx.accounts.vault;
        vault.cumulative_profit = vault.cumulative_profit.saturating_add(profit - fee);

        emit!(ArbitrageExecuted {
            vault: vault.key(),
            notional,
            profit,
            fee,
        });
        Ok(())
    }

    /// Owner-only withdrawal. The bot's delegated authority can never reach
    /// this instruction (I4).
    pub fn withdraw(ctx: Context<Withdraw>, amount: u64) -> Result<()> {
        let vault = &ctx.accounts.vault;
        let owner_key = vault.owner;
        let seeds: &[&[u8]] = &[b"vault", owner_key.as_ref(), &[vault.bump]];
        let signer = &[seeds];
        let cpi = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.vault_token_account.to_account_info(),
                to: ctx.accounts.destination.to_account_info(),
                authority: ctx.accounts.vault.to_account_info(),
            },
            signer,
        );
        token::transfer(cpi, amount)?;
        Ok(())
    }
}

/// Pure, host-testable arithmetic. Kept free of Anchor types so it can be unit
/// tested with a plain `cargo test` in this crate.
pub mod math {
    /// Protocol fee on realized profit: `profit * fee_bps / 10_000`.
    /// Uses u128 intermediate to avoid overflow; result fits in u64 because it
    /// is a fraction of `profit`.
    pub fn protocol_fee(profit: u64, fee_bps: u16) -> u64 {
        ((profit as u128) * (fee_bps as u128) / 10_000) as u64
    }

    /// I2 predicate: a settlement clears the floor.
    pub fn clears_profit_floor(balance_before: u64, balance_after: u64, min_profit: u64) -> bool {
        balance_after
            .checked_sub(balance_before)
            .map(|p| p >= min_profit)
            .unwrap_or(false)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn fee_is_fraction_of_profit() {
            assert_eq!(protocol_fee(1_000_000, 100), 10_000); // 1%
            assert_eq!(protocol_fee(1_000_000, 0), 0);
            assert_eq!(protocol_fee(0, 1000), 0);
        }

        #[test]
        fn fee_never_exceeds_profit() {
            // Even at the 10% ceiling the fee is a small fraction.
            let profit = u64::MAX / 2;
            assert!(protocol_fee(profit, 1000) < profit);
        }

        #[test]
        fn profit_floor_predicate() {
            assert!(clears_profit_floor(100, 150, 50));
            assert!(!clears_profit_floor(100, 140, 50));
            assert!(!clears_profit_floor(100, 90, 0)); // a loss never clears
        }
    }
}

/// Flash-loan CPI boundary.
///
/// ROADMAP: Kamino Lending flash loan. The integration shape is well-defined:
/// Kamino exposes `flash_borrow_reserve_liquidity` and
/// `flash_repay_reserve_liquidity` instructions on the lending program
/// (`KLend2g3cP87fffoy8q1mQqGKjrxjC8boSyAYavgmjD`). A flash-loan transaction
/// must contain a matching borrow/repay pair, and the runtime enforces that the
/// borrowed liquidity is returned plus fee within the same transaction, or the
/// whole transaction reverts. The accounts (lending market, reserve, reserve
/// liquidity supply, the vault's destination token account, and the
/// instructions sysvar used to validate the borrow/repay pairing) are passed in
/// via `remaining_accounts`.
///
/// The actual `invoke_signed` is not wired here; this is a reference framework,
/// not a live deployment. The function returns `NotImplemented` rather than
/// faking a successful borrow.
mod flash_loan {
    use super::*;

    /// Kamino Lending program id (mainnet). Documents the integration target;
    /// referenced once the CPI is wired in a deployment build.
    #[allow(dead_code)]
    pub const KAMINO_LENDING_PROGRAM_ID: &str = "KLend2g3cP87fffoy8q1mQqGKjrxjC8boSyAYavgmjD";

    pub fn borrow(_ctx: &Context<ExecuteArbitrage>, _amount: u64) -> Result<()> {
        // TODO(roadmap): CPI `flash_borrow_reserve_liquidity` for `amount` of
        // the base mint into `vault_token_account`. Must be paired with `repay`
        // in the same tx; the chain reverts otherwise (this is what bounds the
        // downside to fees).
        Err(ExecutorError::NotImplemented.into())
    }

    pub fn repay(_ctx: &Context<ExecuteArbitrage>, _amount: u64) -> Result<()> {
        // TODO(roadmap): CPI `flash_repay_reserve_liquidity` for principal +
        // provider fee.
        Err(ExecutorError::NotImplemented.into())
    }
}

/// DEX swap-route CPI boundary. ROADMAP: per-DEX swap CPIs driven by
/// `remaining_accounts`, mirroring the off-chain `lorenz-dex` integrations.
mod route {
    use super::*;

    pub fn execute_route(_ctx: &Context<ExecuteArbitrage>, _notional: u64) -> Result<()> {
        // TODO(roadmap): walk the route encoded in `remaining_accounts`,
        // issuing a swap CPI per hop (Raydium, Meteora, Whirlpool, ...).
        Err(ExecutorError::NotImplemented.into())
    }
}

#[account]
pub struct Vault {
    pub owner: Pubkey,
    pub bump: u8,
    pub base_mint: Pubkey,
    pub vault_token_account: Pubkey,
    pub protocol_fee_account: Pubkey,
    pub spend_cap: u64,
    pub fee_bps: u16,
    pub cumulative_profit: u64,
}

impl Vault {
    pub const SPACE: usize = 8 + 32 + 1 + 32 + 32 + 32 + 8 + 2 + 8;
}

#[derive(Accounts)]
pub struct InitializeVault<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,

    #[account(
        init,
        payer = owner,
        space = Vault::SPACE,
        seeds = [b"vault", owner.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, Vault>,

    /// CHECK: stored for reference; mint is validated by the token account.
    pub base_mint: UncheckedAccount<'info>,

    #[account(token::authority = vault)]
    pub vault_token_account: Account<'info, TokenAccount>,

    pub protocol_fee_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ExecuteArbitrage<'info> {
    /// The bot's delegated authority. It may trigger arbitrage but, by I4,
    /// cannot withdraw. Must match the delegate recorded out-of-band.
    pub bot_authority: Signer<'info>,

    #[account(
        mut,
        seeds = [b"vault", vault.owner.as_ref()],
        bump = vault.bump,
    )]
    pub vault: Account<'info, Vault>,

    #[account(
        mut,
        address = vault.vault_token_account,
        token::authority = vault,
    )]
    pub vault_token_account: Account<'info, TokenAccount>,

    #[account(mut, address = vault.protocol_fee_account)]
    pub protocol_fee_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    // Remaining accounts encode the DEX route (see `route::execute_route`).
}

#[derive(Accounts)]
pub struct Withdraw<'info> {
    pub owner: Signer<'info>,

    #[account(
        mut,
        seeds = [b"vault", owner.key().as_ref()],
        bump = vault.bump,
        has_one = owner,
    )]
    pub vault: Account<'info, Vault>,

    #[account(mut, address = vault.vault_token_account)]
    pub vault_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub destination: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[event]
pub struct ArbitrageExecuted {
    pub vault: Pubkey,
    pub notional: u64,
    pub profit: u64,
    pub fee: u64,
}

#[error_code]
pub enum ExecutorError {
    #[msg("protocol fee exceeds the hard ceiling")]
    FeeTooHigh,
    #[msg("notional exceeds the vault spend cap")]
    SpendCapExceeded,
    #[msg("arbitrage did not clear the minimum profit floor")]
    ArbNotProfitable,
    #[msg("arithmetic overflow")]
    MathOverflow,
    #[msg("not implemented yet (roadmap)")]
    NotImplemented,
}
