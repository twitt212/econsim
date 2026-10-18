//! Deterministic RNG streams. Every random decision draws from a generator keyed by
//! (world seed, tick, stream, agent id) so results are reproducible regardless of thread scheduling,
//! and baseline/shocked runs share random numbers (common random numbers).

use rand::SeedableRng;
pub use rand::Rng;
use rand_pcg::Pcg64Mcg;

pub type SimRng = Pcg64Mcg;

/// Stream identifiers — one per module/decision so modules don't share draws.
pub mod stream {
    pub const INIT: u64 = 1;
    pub const EXOG: u64 = 2;
    pub const DEMOGRAPHY: u64 = 3;
    pub const LABOR: u64 = 4;
    pub const FIRMS: u64 = 5;
    pub const GOODS: u64 = 6;
    pub const CONSUMPTION: u64 = 7;
    pub const HOUSING: u64 = 8;
    pub const BANKING: u64 = 9;
    pub const MONETARY: u64 = 10;
    pub const FISCAL: u64 = 11;
    pub const ENTRY: u64 = 12;
    /// Household consumption type draws (fix-households). High bits set so they never collide with the ids above.
    pub const HH_TYPE: u64 = 0xF1C5_0000_0000_0013;
    /// Labour: pay draws for firm matches (fix-labour).
    pub const LABOUR_MATCH_PAY: u64 = 0x4D41_5443;
    /// Labour: unemployment-exit draws (fix-labour).
    pub const LABOUR_UE_EXIT: u64 = 0x5545_4558;
    // p0-exog: each new exogenous process has its own stream, so switching one on never shifts another's draws.
    /// Financial risk factor innovations and jumps (exog::fin_factor_step).
    pub const FIN_FACTOR: u64 = 0x5046_4558_0000_0001;
    /// Term-premium level factor innovations.
    pub const TERM_LEVEL: u64 = 0x5045_5450_0000_0002;
    /// Term-premium curvature factor innovations.
    pub const TERM_CURV: u64 = 0x5045_5443_0000_0003;
    /// Dollar risk-premium innovations.
    pub const FX_PSI: u64 = 0x5046_5853_0000_0004;
    /// Oil extensions: Student-t scale draws (the base oil normal stays on EXOG).
    pub const OIL_EXT: u64 = 0x4F49_4C58_0000_0005;
    /// Oil jump counts and sizes, separate from the Student-t scale draws so that changing `oil_t_nu` leaves the jump
    /// path unchanged.
    pub const OIL_JUMP: u64 = 0x4F49_4C4A_0000_0006;
    /// Idiosyncratic house-value deviations (housing.rs `update_house_values`), keyed by house id.
    pub const HOUSE_VALUE: u64 = 13;
    /// Income-shock episodes of leveraged owners (housing.rs `update_income_shocks`), keyed by house id.
    pub const INCOME_SHOCK: u64 = 14;
    /// Re-fit Step 5b: the monthly bankruptcy draw of a distressed firm (firms.rs `firm_default_draw`), keyed by firm.
    pub const FIRM_DEFAULT: u64 = 0x4644_4546_0000_0008;
    /// Re-fit Step 9: stochastic rounding of firms' posted vacancies (recruiting capacity). Separate from FIRMS so the
    /// markup draws stay aligned with the legacy path.
    pub const HIRING: u64 = 0x4849_5245_0000_0007;
    /// b8-world (H13): bloc innovations, one draw set per bloc and month (world::draws). Four normals per bloc, always taken.
    pub const WORLD_BLOC: u64 = 0x574F_524C_0000_0009;
    /// b8-world (H13): oil-specific innovations of the oil market (stage 3).
    pub const WORLD_OIL_I: u64 = 0x574F_524C_0000_000A;
    /// b8-world (H13): oil disruption jumps and OPEC+ surprises (stage 3).
    pub const WORLD_OIL_S: u64 = 0x574F_524C_0000_000B;
    /// b8-world (H13): dollar disconnect noise (stage 4).
    pub const WORLD_FX: u64 = 0x574F_524C_0000_000C;
}

/// Every stream id, for the collision test. Add new ids here when registering a stream.
pub const ALL_STREAMS: &[u64] = &[
    stream::INIT, stream::EXOG, stream::DEMOGRAPHY, stream::LABOR, stream::FIRMS, stream::GOODS,
    stream::CONSUMPTION, stream::HOUSING, stream::BANKING, stream::MONETARY, stream::FISCAL, stream::ENTRY,
    stream::HH_TYPE, stream::LABOUR_MATCH_PAY, stream::LABOUR_UE_EXIT,
    stream::FIN_FACTOR, stream::TERM_LEVEL, stream::TERM_CURV, stream::FX_PSI, stream::OIL_EXT, stream::OIL_JUMP,
    stream::HOUSE_VALUE, stream::INCOME_SHOCK, stream::FIRM_DEFAULT, stream::HIRING,
    stream::WORLD_BLOC, stream::WORLD_OIL_I, stream::WORLD_OIL_S, stream::WORLD_FX,
];

#[inline]
fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// RNG for one agent's decisions in one module in one month.
#[inline]
pub fn agent_rng(seed: u64, tick: u32, stream: u64, id: u64) -> SimRng {
    let k = splitmix(seed ^ splitmix((tick as u64) << 20 ^ stream) ^ splitmix(id.wrapping_add(0xA5A5)));
    Pcg64Mcg::seed_from_u64(k)
}

/// RNG for a module-level (non-agent) decision sequence in one month.
#[inline]
pub fn module_rng(seed: u64, tick: u32, stream: u64) -> SimRng {
    agent_rng(seed, tick, stream, u64::MAX)
}

/// Standard normal draw (Box–Muller), cheap and dependency-light.
#[inline]
pub fn normal<R: Rng + ?Sized>(rng: &mut R) -> f64 {
    let u1: f64 = rng.gen::<f64>().max(1e-12);
    let u2: f64 = rng.gen::<f64>();
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
}

/// Lognormal with E[X] = 1 for the given sigma of the underlying normal.
#[inline]
pub fn lognormal_mean1<R: Rng + ?Sized>(rng: &mut R, sigma: f64) -> f64 {
    (sigma * normal(rng) - 0.5 * sigma * sigma).exp()
}

#[cfg(test)]
mod stream_registry_tests {
    use super::*;

    /// (tick << 20) ^ stream is the stream key, with ticks far below 2^20. Two streams collide iff their XOR has no
    /// bits below bit 20 (a tick difference of 2^20 months would then be needed to separate them).
    #[test]
    fn stream_ids_do_not_collide_for_any_realistic_tick() {
        for (i, &a) in ALL_STREAMS.iter().enumerate() {
            for &b in &ALL_STREAMS[i + 1..] {
                let x = a ^ b;
                assert!(x & 0xF_FFFF != 0 || (x >> 20) >= (1 << 20), "streams {a:#x} and {b:#x} collide");
                assert_ne!(a, b);
            }
        }
    }
}
