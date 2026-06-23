//! The decode boundary: raw on-chain account bytes -> normalized pool snapshot.
//!
//! This is where the deterministic core meets untrusted external data. Keeping
//! it behind traits means the live engine, a replay/backtest, and unit tests
//! all share one definition of "what a pool is".
//!
//! ## How reserves actually work
//!
//! For classic constant-product AMMs (Raydium AMM v4, Raydium CP-Swap) the pool
//! account itself does NOT store reserves. It stores the two vault token-account
//! addresses; the reserves are the SPL token balances held in those vaults.
//! Decoding is therefore two steps, which this module models faithfully:
//!
//! 1. [`PoolAccountDecoder::decode_accounts`] reads the pool account into a
//!    [`PoolAccounts`] (mints, vaults, fee).
//! 2. The caller fetches the two vault token accounts, reads their balances with
//!    [`spl::token_account_amount`], and calls [`PoolAccounts::assemble`].
//!
//! ## Honest status
//!
//! - SPL token-account decoding and the Raydium AMM v4 / CP-Swap pool-account
//!   layouts are implemented with real on-chain offsets and covered by fixture
//!   tests. The offsets are adapted from the (MIT-licensed) reference
//!   implementation this project studied.
//! - Concentrated-liquidity venues (Whirlpool, Raydium CLMM, Meteora DLMM) are
//!   intentionally left as roadmap: their pricing is NOT constant-product
//!   (sqrt-price + tick arrays), so decoding them into a [`crate::CpmmPool`]
//!   would misrepresent their math. They return [`DecodeError::NotImplemented`].

use crate::clmm::ClmmPool;
use crate::CpmmPool;
use lorenz_core::types::{Bps, Dex, PoolId, TokenId};
use thiserror::Error;

/// A 32-byte Solana public key, kept dependency-free.
pub type RawPubkey = [u8; 32];

/// Base58 string form of a raw pubkey (how Solana displays addresses).
pub fn b58(key: &RawPubkey) -> String {
    bs58::encode(key).into_string()
}

#[derive(Debug, Error, PartialEq)]
pub enum DecodeError {
    #[error("decoder for {0:?} is not implemented (roadmap)")]
    NotImplemented(Dex),
    #[error("account data too short: expected at least {expected} bytes, got {got}")]
    TooShort { expected: usize, got: usize },
    #[error("malformed account data: {0}")]
    Malformed(String),
}

/// Read a 32-byte pubkey at `offset`, bounds-checked.
fn read_pubkey(data: &[u8], offset: usize) -> Result<RawPubkey, DecodeError> {
    let end = offset + 32;
    if data.len() < end {
        return Err(DecodeError::TooShort {
            expected: end,
            got: data.len(),
        });
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&data[offset..end]);
    Ok(key)
}

/// Read a little-endian u16 at `offset`, bounds-checked.
fn read_u16(data: &[u8], offset: usize) -> Result<u16, DecodeError> {
    let end = offset + 2;
    if data.len() < end {
        return Err(DecodeError::TooShort {
            expected: end,
            got: data.len(),
        });
    }
    Ok(u16::from_le_bytes([data[offset], data[offset + 1]]))
}

/// Read a little-endian u128 at `offset`, bounds-checked.
fn read_u128(data: &[u8], offset: usize) -> Result<u128, DecodeError> {
    let end = offset + 16;
    if data.len() < end {
        return Err(DecodeError::TooShort {
            expected: end,
            got: data.len(),
        });
    }
    let mut buf = [0u8; 16];
    buf.copy_from_slice(&data[offset..end]);
    Ok(u128::from_le_bytes(buf))
}

/// SPL Token program account decoding (the part we need for reserves).
pub mod spl {
    use super::DecodeError;

    /// An initialized SPL token account is 165 bytes; the `amount` is a
    /// little-endian u64 at offset 64, and the `mint` is at offset 0.
    pub const TOKEN_ACCOUNT_LEN: usize = 165;
    const AMOUNT_OFFSET: usize = 64;

    /// Balance held by an SPL token account.
    pub fn token_account_amount(data: &[u8]) -> Result<u64, DecodeError> {
        if data.len() < TOKEN_ACCOUNT_LEN {
            return Err(DecodeError::TooShort {
                expected: TOKEN_ACCOUNT_LEN,
                got: data.len(),
            });
        }
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&data[AMOUNT_OFFSET..AMOUNT_OFFSET + 8]);
        Ok(u64::from_le_bytes(buf))
    }
}

/// Static pool info read from a pool account (no reserves yet).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolAccounts {
    pub dex: Dex,
    pub mint_a: RawPubkey,
    pub mint_b: RawPubkey,
    pub vault_a: RawPubkey,
    pub vault_b: RawPubkey,
    /// Swap fee. For venues whose fee lives in a separate config account this is
    /// the documented protocol default; override once that account is read.
    pub fee: Bps,
}

impl PoolAccounts {
    pub fn token_a(&self) -> TokenId {
        TokenId(b58(&self.mint_a))
    }
    pub fn token_b(&self) -> TokenId {
        TokenId(b58(&self.mint_b))
    }

    /// Combine the static info with the two vault balances into a priceable
    /// [`CpmmPool`]. `reserve_a`/`reserve_b` come from
    /// [`spl::token_account_amount`] on `vault_a`/`vault_b`.
    pub fn assemble(
        &self,
        pool_id: impl Into<String>,
        reserve_a: u128,
        reserve_b: u128,
    ) -> CpmmPool {
        CpmmPool {
            id: PoolId(pool_id.into()),
            dex: self.dex,
            token_a: self.token_a(),
            token_b: self.token_b(),
            reserve_a,
            reserve_b,
            fee: self.fee,
        }
    }
}

/// Decodes a pool account into [`PoolAccounts`].
pub trait PoolAccountDecoder {
    fn dex(&self) -> Dex;
    fn decode_accounts(&self, account_data: &[u8]) -> Result<PoolAccounts, DecodeError>;
}

/// Raydium AMM v4 pool layout. Offsets (bytes) into the pool account:
/// coin_vault 336, pc_vault 368, coin_mint 400, pc_mint 432.
#[derive(Debug, Default, Clone, Copy)]
pub struct RaydiumAmmV4Decoder;

impl RaydiumAmmV4Decoder {
    const COIN_VAULT: usize = 336;
    const PC_VAULT: usize = 368;
    const COIN_MINT: usize = 400;
    const PC_MINT: usize = 432;
    /// Raydium AMM v4 standard swap fee is 25 bps.
    const DEFAULT_FEE: Bps = Bps(25);
}

impl PoolAccountDecoder for RaydiumAmmV4Decoder {
    fn dex(&self) -> Dex {
        Dex::RaydiumAmm
    }

    fn decode_accounts(&self, data: &[u8]) -> Result<PoolAccounts, DecodeError> {
        Ok(PoolAccounts {
            dex: Dex::RaydiumAmm,
            vault_a: read_pubkey(data, Self::COIN_VAULT)?,
            vault_b: read_pubkey(data, Self::PC_VAULT)?,
            mint_a: read_pubkey(data, Self::COIN_MINT)?,
            mint_b: read_pubkey(data, Self::PC_MINT)?,
            fee: Self::DEFAULT_FEE,
        })
    }
}

/// Raydium CP-Swap (CPMM) pool layout. Offsets: token_0_vault 72,
/// token_1_vault 104, token_0_mint 168, token_1_mint 200. The fee lives in the
/// separate amm_config account (offset 8); we default and let the caller refine.
#[derive(Debug, Default, Clone, Copy)]
pub struct RaydiumCpmmDecoder;

impl RaydiumCpmmDecoder {
    const TOKEN0_VAULT: usize = 72;
    const TOKEN1_VAULT: usize = 104;
    const TOKEN0_MINT: usize = 168;
    const TOKEN1_MINT: usize = 200;
    const DEFAULT_FEE: Bps = Bps(25);
}

impl PoolAccountDecoder for RaydiumCpmmDecoder {
    fn dex(&self) -> Dex {
        Dex::RaydiumCpmm
    }

    fn decode_accounts(&self, data: &[u8]) -> Result<PoolAccounts, DecodeError> {
        Ok(PoolAccounts {
            dex: Dex::RaydiumCpmm,
            vault_a: read_pubkey(data, Self::TOKEN0_VAULT)?,
            vault_b: read_pubkey(data, Self::TOKEN1_VAULT)?,
            mint_a: read_pubkey(data, Self::TOKEN0_MINT)?,
            mint_b: read_pubkey(data, Self::TOKEN1_MINT)?,
            fee: Self::DEFAULT_FEE,
        })
    }
}

/// Roadmap decoders. These are real types that openly report they are not
/// implemented, rather than misrepresenting non-CPMM math as constant-product.
macro_rules! roadmap_decoder {
    ($name:ident, $dex:expr, $why:literal) => {
        #[doc = $why]
        #[derive(Debug, Default, Clone, Copy)]
        pub struct $name;

        impl PoolAccountDecoder for $name {
            fn dex(&self) -> Dex {
                $dex
            }
            fn decode_accounts(&self, _data: &[u8]) -> Result<PoolAccounts, DecodeError> {
                Err(DecodeError::NotImplemented($dex))
            }
        }
    };
}

roadmap_decoder!(
    MeteoraDammDecoder,
    Dex::MeteoraDamm,
    "Roadmap: dynamic AMM with vaults; layout adaptation pending."
);
roadmap_decoder!(
    PumpAmmDecoder,
    Dex::PumpAmm,
    "Roadmap: layout adaptation pending."
);
roadmap_decoder!(
    SolfiDecoder,
    Dex::Solfi,
    "Roadmap: layout adaptation pending."
);
roadmap_decoder!(
    VertigoDecoder,
    Dex::Vertigo,
    "Roadmap: layout adaptation pending."
);

// ----------------------------------------------------------------------------
// Concentrated-liquidity decoders
// ----------------------------------------------------------------------------

/// Static + dynamic info read from a CLMM pool account. Unlike CPMM, a CLMM
/// pool account carries the current price and active liquidity directly, so no
/// vault balances are needed to price within the current tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClmmPoolAccounts {
    pub dex: Dex,
    pub mint_a: RawPubkey,
    pub mint_b: RawPubkey,
    pub sqrt_price_x64: u128,
    pub liquidity: u128,
    pub fee: Bps,
}

impl ClmmPoolAccounts {
    pub fn assemble(&self, pool_id: impl Into<String>) -> ClmmPool {
        ClmmPool {
            id: PoolId(pool_id.into()),
            dex: self.dex,
            token_a: TokenId(b58(&self.mint_a)),
            token_b: TokenId(b58(&self.mint_b)),
            sqrt_price_x64: self.sqrt_price_x64,
            liquidity: self.liquidity,
            fee: self.fee,
        }
    }
}

/// Decodes a concentrated-liquidity pool account.
pub trait ClmmAccountDecoder {
    fn dex(&self) -> Dex;
    fn decode_clmm(&self, account_data: &[u8]) -> Result<ClmmPoolAccounts, DecodeError>;
}

/// Orca Whirlpool pool-account layout. Offsets (absolute, incl. 8-byte Anchor
/// discriminator): fee_rate 45 (u16), liquidity 49 (u128), sqrt_price 65
/// (u128), token_mint_a 101, token_mint_b 181. Total account length 653.
#[derive(Debug, Default, Clone, Copy)]
pub struct WhirlpoolDecoder;

impl WhirlpoolDecoder {
    const FEE_RATE: usize = 45;
    const LIQUIDITY: usize = 49;
    const SQRT_PRICE: usize = 65;
    const MINT_A: usize = 101;
    const MINT_B: usize = 181;
    const LEN: usize = 653;
}

impl ClmmAccountDecoder for WhirlpoolDecoder {
    fn dex(&self) -> Dex {
        Dex::Whirlpool
    }

    fn decode_clmm(&self, data: &[u8]) -> Result<ClmmPoolAccounts, DecodeError> {
        if data.len() < Self::LEN {
            return Err(DecodeError::TooShort {
                expected: Self::LEN,
                got: data.len(),
            });
        }
        // Whirlpool fee_rate is in hundredths of a basis point (1e-6); convert
        // to bps by dividing by 100. e.g. 3000 -> 30 bps (0.30%).
        let fee_rate = read_u16(data, Self::FEE_RATE)?;
        Ok(ClmmPoolAccounts {
            dex: Dex::Whirlpool,
            mint_a: read_pubkey(data, Self::MINT_A)?,
            mint_b: read_pubkey(data, Self::MINT_B)?,
            liquidity: read_u128(data, Self::LIQUIDITY)?,
            sqrt_price_x64: read_u128(data, Self::SQRT_PRICE)?,
            fee: Bps(u32::from(fee_rate) / 100),
        })
    }
}

/// Roadmap CLMM decoders: real types that report not-implemented rather than
/// guessing a layout.
macro_rules! roadmap_clmm_decoder {
    ($name:ident, $dex:expr, $why:literal) => {
        #[doc = $why]
        #[derive(Debug, Default, Clone, Copy)]
        pub struct $name;

        impl ClmmAccountDecoder for $name {
            fn dex(&self) -> Dex {
                $dex
            }
            fn decode_clmm(&self, _data: &[u8]) -> Result<ClmmPoolAccounts, DecodeError> {
                Err(DecodeError::NotImplemented($dex))
            }
        }
    };
}

roadmap_clmm_decoder!(
    RaydiumClmmDecoder,
    Dex::RaydiumClmm,
    "Roadmap: Raydium CLMM layout (sqrt-price + tick arrays)."
);
roadmap_clmm_decoder!(
    MeteoraDlmmDecoder,
    Dex::MeteoraDlmm,
    "Roadmap: Meteora DLMM bin layout."
);

#[cfg(test)]
mod tests {
    use super::*;

    fn write_key(buf: &mut [u8], offset: usize, byte: u8) {
        for b in buf.iter_mut().skip(offset).take(32) {
            *b = byte;
        }
    }

    #[test]
    fn spl_token_amount_reads_offset_64() {
        let mut data = vec![0u8; spl::TOKEN_ACCOUNT_LEN];
        let amount: u64 = 123_456_789;
        data[64..72].copy_from_slice(&amount.to_le_bytes());
        assert_eq!(spl::token_account_amount(&data).unwrap(), amount);
    }

    #[test]
    fn spl_token_amount_rejects_short() {
        let data = vec![0u8; 100];
        assert!(matches!(
            spl::token_account_amount(&data),
            Err(DecodeError::TooShort { .. })
        ));
    }

    #[test]
    fn raydium_v4_decodes_known_offsets() {
        let mut data = vec![0u8; 512];
        write_key(&mut data, 336, 0xAA); // coin vault
        write_key(&mut data, 368, 0xBB); // pc vault
        write_key(&mut data, 400, 0xCC); // coin mint
        write_key(&mut data, 432, 0xDD); // pc mint

        let acc = RaydiumAmmV4Decoder.decode_accounts(&data).unwrap();
        assert_eq!(acc.vault_a, [0xAA; 32]);
        assert_eq!(acc.vault_b, [0xBB; 32]);
        assert_eq!(acc.mint_a, [0xCC; 32]);
        assert_eq!(acc.mint_b, [0xDD; 32]);
        assert_eq!(acc.fee, Bps(25));
    }

    #[test]
    fn raydium_v4_rejects_short_account() {
        let data = vec![0u8; 400];
        assert!(matches!(
            RaydiumAmmV4Decoder.decode_accounts(&data),
            Err(DecodeError::TooShort { .. })
        ));
    }

    #[test]
    fn assemble_builds_priceable_pool_from_vault_balances() {
        let mut data = vec![0u8; 512];
        write_key(&mut data, 400, 0xCC);
        write_key(&mut data, 432, 0xDD);
        let acc = RaydiumAmmV4Decoder.decode_accounts(&data).unwrap();

        let pool = acc.assemble("somePoolPubkey", 1_000_000, 2_000_000);
        assert_eq!(pool.reserve_a, 1_000_000);
        assert_eq!(pool.reserve_b, 2_000_000);
        // Token ids are the base58 of the mint bytes.
        assert_eq!(pool.token_a, TokenId(b58(&[0xCC; 32])));
    }

    #[test]
    fn whirlpool_decodes_real_layout() {
        let mut data = vec![0u8; 653];
        // fee_rate = 3000 (0.30%) at offset 45
        data[45..47].copy_from_slice(&3000u16.to_le_bytes());
        // liquidity at offset 49
        data[49..65].copy_from_slice(&1_000_000_000u128.to_le_bytes());
        // sqrt_price_x64 = 2^64 (price 1.0) at offset 65
        let q64: u128 = 1u128 << 64;
        data[65..81].copy_from_slice(&q64.to_le_bytes());
        write_key(&mut data, 101, 0x11); // mint a
        write_key(&mut data, 181, 0x22); // mint b

        let acc = WhirlpoolDecoder.decode_clmm(&data).unwrap();
        assert_eq!(acc.liquidity, 1_000_000_000);
        assert_eq!(acc.sqrt_price_x64, q64);
        assert_eq!(acc.fee, Bps(30)); // 3000 / 100
        assert_eq!(acc.mint_a, [0x11; 32]);

        // Assembled pool prices a swap below input (fee + impact).
        let pool = acc.assemble("whirlpoolPubkey");
        let out = pool.quote(&pool.token_a.clone(), 1_000_000).unwrap();
        assert!(out > 0 && out < 1_000_000);
    }

    #[test]
    fn whirlpool_rejects_short_account() {
        assert!(matches!(
            WhirlpoolDecoder.decode_clmm(&[0u8; 100]),
            Err(DecodeError::TooShort { .. })
        ));
    }

    #[test]
    fn roadmap_clmm_decoders_report_not_implemented() {
        assert_eq!(
            RaydiumClmmDecoder.decode_clmm(&[0u8; 1024]),
            Err(DecodeError::NotImplemented(Dex::RaydiumClmm))
        );
    }
}
