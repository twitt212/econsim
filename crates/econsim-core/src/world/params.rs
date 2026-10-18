//! World-block parameters (spec 1.1, 1.7, 1.9): the two embedded JSON files, loaded once through a `OnceLock`, and the
//! effective run-time coefficients derived from them and from the `world_*` config keys.
//!
//! `data/world_params.json` holds the monthly coefficients, steady states, shock sizes, Okun coefficients, the network
//! matrices, the financial persistences and the two OPEC+ composition sets. `data/bloc_solution.json` holds, per bloc, the
//! period-0 map C (5 x 33), the kernels IR[eq][k] for k < 240 and the tail sums, and the pin tables. Both are written by
//! `scripts/world/build_world_params.py` and `scripts/world/solve_blocs.py`.

use std::sync::{Arc, OnceLock};

use serde::Deserialize;

use super::{Bloc, K, NB, NS};
use crate::config::Config;

const PARAMS_JSON: &str = include_str!("data/world_params.json");
const SOLUTION_JSON: &str = include_str!("data/bloc_solution.json");

/// Pin-table lengths of the solution file (rule innovations that hold the rate 1.0 above the rule for L months).
pub const PIN_LENGTHS: [usize; 5] = [3, 6, 12, 18, 24];

#[derive(Debug, Clone, Deserialize)]
pub struct Meta {
    pub calibration_hash: String,
    pub k: usize,
    pub k_tail: usize,
    pub t_kernel: usize,
    pub t_pin: usize,
    pub blocs: Vec<String>,
    pub pstar_sha: String,
}

/// Monthly coefficients (`monthly_coefficients` of the research, full precision; theta before the P* scale).
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Monthly {
    pub b1: f64,
    pub b2: f64,
    pub b3: f64,
    pub b4: f64,
    pub b5: f64,
    pub l1: f64,
    pub l2: f64,
    pub l3: f64,
    pub g1: f64,
    pub g2: f64,
    pub g4: f64,
    pub phi: f64,
    pub uip_w: f64,
    pub theta: f64,
}

/// P* table row (`scripts/world/pstar.json`, resolved).
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct PStar {
    pub theta_scale: f64,
    pub lam_rp: f64,
    pub k_us: f64,
    pub g_z: f64,
    pub uip_w: f64,
    pub oil_is_scale: f64,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Sd {
    pub demand: f64,
    pub cost_push: f64,
    pub policy: f64,
    pub risk_premium: f64,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Okun {
    pub a1: f64,
    pub a2: f64,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct OilCoef {
    pub is_coef: f64,
    pub energy_cpi_weight: f64,
    pub phi_fuel: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BlocParams {
    pub name: String,
    pub inflation_target: f64,
    pub neutral_real_rate: f64,
    pub policy_rate_floor: f64,
    pub monthly: Monthly,
    pub pstar: PStar,
    pub sd: Sd,
    pub okun: Option<Okun>,
    pub oil: OilCoef,
    pub weight_ppp: f64,
    pub weight_usd: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Network {
    pub s_us: Vec<f64>,
    pub s_bb: Vec<Vec<f64>>,
    pub w_us: Vec<f64>,
    pub w_bb: Vec<Vec<f64>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Financial {
    pub rho_g: f64,
    pub rho_rp: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Weights {
    pub weight_usd: Vec<f64>,
    pub weight_ppp: Vec<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Opec {
    pub uae_in: Weights,
    pub uae_out: Weights,
    pub uae_gdp_usd_bn_2024: f64,
    pub uae_gdp_ppp_bn_2024: f64,
}

/// Trade tables (stage 2, `build_world_params.py` `build_trade`): the 2024 bilateral goods trade of the US with the twelve blocs
/// by model sector (USD bn), the bloc-to-bloc import shares, the USD-invoicing shares, the dollar-weight vector of the H10
/// index, and the scalar pass-through and elasticity values read from the trade research. Bloc rows in bloc order; sector
/// columns in model sector order (Energy ... Construction).
#[derive(Debug, Clone, Deserialize)]
pub struct Trade {
    pub gdp_2024_bn: f64,
    pub sectors: Vec<String>,
    pub theta: Vec<f64>,
    pub imports_base: Vec<Vec<f64>>,
    pub exports_base: Vec<Vec<f64>>,
    /// import_shares[k][j] = bloc j's import share of bloc k (research `bloc_goods_matrix.import_shares_2024[k][j]`, exporter
    /// rows, importer columns; the US is not in the twelve).
    pub import_shares: Vec<Vec<f64>>,
    /// Normalised H10 broad-dollar weights (research `finance dollar.h10_broad_weights_pct_2026`, divided by their sum).
    pub dollar_weights: Vec<f64>,
    pub dollar_weights_sum_raw: f64,
    pub dcp_kappa_usd: f64,
    pub dcp_kappa_nonusd: f64,
    pub export_fx_sr: f64,
    pub foreign_income_elasticity: f64,
    pub origin_substitution_lr: f64,
    /// Stage 4-3 (spec 2.5, D9): the retaliation rule tables (trade `retaliation.recommended_rule`), bloc order, sector order.
    #[serde(default)]
    pub retaliation: RetTables,
}

/// Stage 4-3 (spec 1.7, 2.5): kappa_j, the lag in months, the cap of the duty and the sector weights w_s of the retaliation rule.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RetTables {
    pub kappa: Vec<f64>,
    pub lag: Vec<usize>,
    pub cap: Vec<f64>,
    pub w_s: Vec<f64>,
}

/// Stage 4-3 (spec 1.7, 2.8): the cross-border position tables, real-world $bn by bloc (finance `cross_border`), the two ledger-stock
/// shares, the equity loadings, the foreign-currency shares and the implied yields (percent).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PosTables {
    pub a_eq: Vec<f64>,
    pub a_debt: Vec<f64>,
    pub a_fdi: Vec<f64>,
    pub a_bank: Vec<f64>,
    pub l_agcorp: Vec<f64>,
    pub l_eq: Vec<f64>,
    pub l_fdi: Vec<f64>,
    pub tic_share: Vec<f64>,
    pub dep_share: Vec<f64>,
    pub equity_loading: Vec<f64>,
    pub fc_eq: f64,
    pub fc_debt: f64,
    pub fc_fdi: f64,
    pub fc_bank: f64,
    pub fc_liab: f64,
    pub yield_assets_pct: f64,
    pub yield_liab_pct: f64,
}

#[derive(Deserialize)]
struct RawParams {
    meta: Meta,
    blocs: Vec<BlocParams>,
    network: Network,
    financial: Financial,
    opec: Opec,
    trade: Trade,
    oil: super::oil::OilTables,
    #[serde(default)]
    finance: Finance,
}

/// One bloc's offline solution: C (rows = y, pi, i, z, mrr; columns = the 33-state), the kernels per equation
/// (`ir[eq][k][o]`, eq 0 IS, 1 PC, 2 rule, 3 UIP), the tail sums, and the pin tables (index by `PIN_LENGTHS`).
#[derive(Debug, Clone)]
pub struct BlocSolution {
    pub c: [[f64; NS]; 5],
    pub ir: [Vec<[f64; 5]>; 4],
    pub tail: [[f64; 5]; 4],
    pub pin: [Vec<f64>; 5],
}

#[derive(Deserialize)]
struct RawKernels {
    is: Vec<Vec<f64>>,
    pc: Vec<Vec<f64>>,
    rule: Vec<Vec<f64>>,
    v: Vec<Vec<f64>>,
}

#[derive(Deserialize)]
struct RawTail {
    is: Vec<f64>,
    pc: Vec<f64>,
    rule: Vec<f64>,
    v: Vec<f64>,
}

#[derive(Deserialize)]
struct RawBlocSolution {
    c: Vec<Vec<f64>>,
    ir: RawKernels,
    tail: RawTail,
    pin: std::collections::BTreeMap<String, Vec<f64>>,
}

/// Meta of `bloc_solution.json` (the calibration hash it shares with `world_params.json`).
#[derive(Deserialize)]
struct SolMeta {
    calibration_hash: String,
}

#[derive(Deserialize)]
struct RawSolution {
    meta: SolMeta,
    blocs: std::collections::BTreeMap<String, RawBlocSolution>,
}

fn arr5(v: &[f64]) -> [f64; 5] {
    v.try_into().expect("5 outputs")
}

fn kernels(rows: &[Vec<f64>]) -> Vec<[f64; 5]> {
    assert_eq!(rows.len(), K, "kernel length");
    rows.iter().map(|r| arr5(r)).collect()
}

/// The complete parameter set: the tables of both files, in bloc order EA, CN, JP, UK, CA, MX, IN, KR, BR, AU, OPEC+, RoW.
#[derive(Debug)]
pub struct WorldParams {
    pub meta: Meta,
    pub blocs: Vec<BlocParams>,
    pub sol: Vec<BlocSolution>,
    pub network: Network,
    pub financial: Financial,
    pub opec: Opec,
    /// Row sums of `w_bb` (sum over the 11 non-US partners, = 1 - w_us to 1e-4): the Peff addend uses (sum - 1).
    pub wsum: [f64; NB],
    pub trade: Trade,
    /// Oil market tables (stage 3, spec 2.4): regions, supply, inventories, shocks, OPEC+ and the profiles.
    pub oil: super::oil::OilTables,
    /// Stage 4-1 (spec 1.7, 2.6): the financial tables (risk-off loadings of the dollar, by bloc).
    pub finance: Finance,
}

/// Stage 4-1 (spec 1.7): the finance tables of the dollar risk term, copied from `finance.dollar` of the research params by
/// `scripts/world/build_world_params.py`. `gamma_pct_per_vix[j]` = % dollar appreciation per VIX point of bloc j (bloc order).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Finance {
    #[serde(default)]
    pub gamma_pct_per_vix: Vec<f64>,
    /// Stage 4-2 (spec 1.7, 2.7): TIC-recorded Treasury holdings by bloc, $bn (finance `treasury_demand.foreign_holdings_ust_bn_2026_07`).
    #[serde(default)]
    pub tic_ust_bn: Vec<f64>,
    /// Stage 4-2: the official share of each bloc's holdings (`official_share_of_bloc_ust_holdings`).
    #[serde(default)]
    pub official_share: Vec<f64>,
    /// Stage 4-2: the sourced official total, $bn (`foreign_official_ust_bn`, 3773.1).
    #[serde(default)]
    pub foreign_official_ust_bn: f64,
    /// Stage 4-3 (spec 1.7, 2.8): the memo position tables.
    #[serde(default)]
    pub positions: PosTables,
}

impl WorldParams {
    /// The embedded tables, parsed once and shared.
    pub fn load() -> Arc<WorldParams> {
        static CELL: OnceLock<Arc<WorldParams>> = OnceLock::new();
        CELL.get_or_init(|| Arc::new(WorldParams::parse())).clone()
    }

    fn parse() -> WorldParams {
        let raw: RawParams = serde_json::from_str(PARAMS_JSON).expect("world_params.json parses");
        let sraw: RawSolution = serde_json::from_str(SOLUTION_JSON).expect("bloc_solution.json parses");
        assert_eq!(raw.blocs.len(), NB, "12 blocs");
        assert_eq!(raw.meta.blocs.len(), NB);
        assert_eq!(sraw.meta.calibration_hash, raw.meta.calibration_hash, "both files come from one calibration");
        let mut sol = Vec::with_capacity(NB);
        for (i, b) in raw.blocs.iter().enumerate() {
            assert_eq!(b.name, Bloc::ALL[i].name(), "bloc order");
            let s = sraw.blocs.get(&b.name).unwrap_or_else(|| panic!("solution for {}", b.name));
            let c: Vec<Vec<f64>> = s.c.clone();
            assert_eq!(c.len(), 5);
            let mut cm = [[0.0; NS]; 5];
            for (o, row) in c.iter().enumerate() {
                assert_eq!(row.len(), NS, "C row length");
                cm[o].copy_from_slice(row);
            }
            let ir = [kernels(&s.ir.is), kernels(&s.ir.pc), kernels(&s.ir.rule), kernels(&s.ir.v)];
            let tail = [arr5(&s.tail.is), arr5(&s.tail.pc), arr5(&s.tail.rule), arr5(&s.tail.v)];
            let pin: [Vec<f64>; 5] = std::array::from_fn(|j| {
                let p = s.pin.get(&PIN_LENGTHS[j].to_string()).unwrap_or_else(|| panic!("pin {}", PIN_LENGTHS[j])).clone();
                assert_eq!(p.len(), PIN_LENGTHS[j], "pin length");
                p
            });
            sol.push(BlocSolution { c: cm, ir, tail, pin });
        }
        let wsum: [f64; NB] = std::array::from_fn(|i| raw.network.w_bb[i].iter().sum());
        WorldParams {
            meta: raw.meta,
            blocs: raw.blocs,
            sol,
            network: raw.network,
            financial: raw.financial,
            opec: raw.opec,
            wsum,
            trade: raw.trade,
            oil: raw.oil,
            finance: raw.finance,
        }
    }
}

/// Effective run-time coefficients (spec 1.7 and 2.2.2): the P* table times the `world_*` multipliers, plus the
/// projection persistences and the M-F bound. Computed once per world from the config.
#[derive(Debug, Clone, Copy)]
pub struct Par {
    /// theta_i = monthly.theta x pstar.theta_scale x world_theta_mult.
    pub theta: [f64; NB],
    /// k_us,i = pstar.k_us x world_kus_mult.
    pub k_us: [f64; NB],
    /// lambda_rp,i = pstar.lam_rp x world_lam_rp_mult.
    pub lam_rp: [f64; NB],
    /// beta^oil_i = oil.is_coef x pstar.oil_is_scale x world_oil_is_mult.
    pub beta_oil: [f64; NB],
    /// beta^q_i (OPEC+ only) = world_opec_va_share (1 - b1 - b2).
    pub beta_q: [f64; NB],
    /// Scale of the stochastic innovations (`world_shock_scale`).
    pub shock_scale: f64,
    pub rho_f: f64,
    pub rho_p: f64,
    pub rho_oil: f64,
    pub rho_qty: f64,
    pub okun_ratio: f64,
    /// pp, subtracted from the US policy deviation (D28).
    pub ius_offset: f64,
    /// pp, subtracted from the US output gap before it enters the blocs (N1, `world_yus_offset`).
    pub yus_offset: f64,
    /// Decay of the offset along the expected path (0.97 unless the rule path is customised).
    pub ex_decay: f64,
    /// b7-transmission M-F knee and saturation scale (`fx_rate_knee`, `fx_rate_sat_scale`), decimal.
    pub knee: f64,
    pub sat: f64,
    /// Stage 4-1 (spec 2.7): the financial-factor loading of bloc i is theta_i x `theta_f_ratio` / `vix_per_100bp` per VIX point.
    pub theta_f_ratio: f64,
    /// VIX points per 100 bp (`world_vix_per_100bp`).
    pub vix_per_100bp: f64,
    /// Persistence of the financial factor's demand channel (`fin_factor_rho`).
    pub fin_rho: f64,
    /// Multiplier on the bloc risk-premium innovation sd (`world_fx_rp_sd_mult`, applied in `draws`).
    pub rp_sd_mult: f64,
    /// List B (spec 2.2.2, rev 9, D48): knee and scale of the bound on the total US-rate excess return `ex + lambda_rp gp` in the
    /// bloc dollar equation, decimal. `rp_sat` is zero (off) unless `world_link_dollar` is on.
    pub rp_knee: f64,
    pub rp_sat: f64,
}

impl Par {
    pub fn from_config(cfg: &Config, p: &WorldParams) -> Par {
        let mut par = Par {
            theta: [0.0; NB],
            k_us: [0.0; NB],
            lam_rp: [0.0; NB],
            beta_oil: [0.0; NB],
            beta_q: [0.0; NB],
            shock_scale: cfg.world_shock_scale,
            rho_f: cfg.world_rho_f,
            rho_p: cfg.world_rho_p,
            rho_oil: cfg.world_rho_oil,
            rho_qty: cfg.world_rho_qty,
            okun_ratio: cfg.world_okun_ratio,
            ius_offset: cfg.world_ius_offset,
            yus_offset: cfg.world_yus_offset,
            ex_decay: rule_path_decay(cfg),
            knee: cfg.fx_rate_knee,
            sat: cfg.fx_rate_sat_scale,
            theta_f_ratio: cfg.world_theta_f_ratio,
            vix_per_100bp: cfg.world_vix_per_100bp,
            fin_rho: cfg.fin_factor_rho,
            rp_sd_mult: cfg.world_fx_rp_sd_mult,
            rp_knee: cfg.world_rp_sat_knee,
            rp_sat: if cfg.world_link_dollar { cfg.world_rp_sat_scale } else { 0.0 },
        };
        for i in 0..NB {
            let b = &p.blocs[i];
            par.theta[i] = b.monthly.theta * b.pstar.theta_scale * cfg.world_theta_mult;
            par.k_us[i] = b.pstar.k_us * cfg.world_kus_mult;
            par.lam_rp[i] = b.pstar.lam_rp * cfg.world_lam_rp_mult;
            par.beta_oil[i] = b.oil.is_coef * b.pstar.oil_is_scale * cfg.world_oil_is_mult;
            if i == Bloc::OpecPlus.idx() {
                par.beta_q[i] = cfg.world_opec_va_share * (1.0 - b.monthly.b1 - b.monthly.b2);
            }
        }
        par
    }
}

/// The decay of the US-policy offset along the expected path: 0.97, or `rule_path_decay` when the rule path is customised
/// (the same condition as `monetary::floored_expected`).
fn rule_path_decay(cfg: &Config) -> f64 {
    let custom = cfg.rule_path_weight > 0.0 && cfg.rule_path_decay.is_finite() && cfg.rule_path_decay != 0.97;
    if custom {
        cfg.rule_path_decay.clamp(0.0, 0.999)
    } else {
        0.97
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_embedded_json_parses() {
        let p = WorldParams::load();
        assert_eq!(p.blocs.len(), NB);
        assert_eq!(p.meta.k, K);
        assert_eq!(p.meta.k_tail, 480);
        assert_eq!(p.meta.t_kernel, 720);
        assert_eq!(p.meta.blocs.len(), NB);
        assert!(p.meta.calibration_hash.len() == 64);
        assert_eq!(p.network.s_bb.len(), NB);
        assert_eq!(p.network.w_bb.len(), NB);
        assert!(p.financial.rho_g > 0.0 && p.financial.rho_g < 1.0);
        assert_eq!(p.trade.theta.len(), NB);
        assert_eq!(p.trade.imports_base.len(), NB);
        assert!(p.trade.imports_base.iter().all(|r| r.len() == crate::types::N_SECTORS));
        assert_eq!(p.trade.import_shares.len(), NB);
        assert_eq!(p.trade.dollar_weights.len(), NB);
    }
}
