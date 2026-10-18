//! World-block metrics (spec 1.6 `Metrics.world`). Stage 1 ships the first group; every array is in bloc order
//! EA, CN, JP, UK, CA, MX, IN, KR, BR, AU, OPEC+, RoW. Serialised only when the block is on (`Metrics::world` is `None`
//! otherwise and the key is skipped).

use serde::{Deserialize, Serialize};

use super::NB;

fn all_zero(a: &[f64; NB]) -> bool {
    a.iter().all(|x| *x == 0.0)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct WorldMetrics {
    /// % output gap.
    pub y_gap: [f64; NB],
    /// Annualised pp, deviation of inflation from the bloc's target.
    pub pi_gap: [f64; NB],
    /// pp, deviation of the policy rate from its steady state.
    pub i_gap: [f64; NB],
    /// % real exchange rate against the dollar (plus = depreciation).
    pub z: [f64; NB],
    /// Level, annualised pp: inflation target plus pi_gap.
    pub infl: [f64; NB],
    /// Level, pp: neutral real rate + target + i_gap, floored at the policy-rate floor.
    pub policy: [f64; NB],
    /// exp(ln S_j - base): foreign currency per USD relative to the first reported month (1.0 there).
    pub s_index: [f64; NB],
    /// exp(ln P_j - base): price level relative to the first reported month (1.0 there).
    pub p_index: [f64; NB],
    /// Level, annualised pp (equal to `infl` in stage 1: the oil gap is zero).
    pub headline: [f64; NB],
    /// pp, Okun unemployment gap from the bloc's own state (zero for OPEC+ and RoW, which have no Okun data).
    pub u_gap: [f64; NB],
    // --- stage 2 (trade; zero before the base, spec 1.6). Serde defaults keep older dumps readable. ---
    /// Annualised $bn: customs imports from each origin (the origin shares times the month's customs imports by sector).
    #[serde(default)]
    pub imports_bn: [f64; NB],
    /// Annualised $bn: exports to each bloc (the realised sector exports allocated by the export shares of spec 2.5).
    #[serde(default)]
    pub exports_bn: [f64; NB],
    /// % origin log price r_j (dominant-currency pass-through of the real exchange rate; zero before the base).
    #[serde(default)]
    pub r_import: [f64; NB],
    /// Effective duty per channel, decimal (household: retail and CPI wedges; firm: inputs and equipment).
    #[serde(default)]
    pub tau_eff_hh: f64,
    #[serde(default)]
    pub tau_eff_firm: f64,
    /// Duty by origin, decimal (memo).
    #[serde(default)]
    pub tau_by_origin: [f64; NB],
    /// Real imports and real exports, 100 at the month before the base (nominal over the tradables price index, relative).
    /// `export_volume_idx` is an addition to spec 1.6 (needed for the W2-02 real-export row).
    #[serde(default)]
    pub import_volume_idx: f64,
    #[serde(default)]
    pub export_volume_idx: f64,
    /// Import price index (100 at the base), CPI-weighted over the import shares (the metric `import_price_index` before duties).
    #[serde(default)]
    pub import_price_idx: f64,
    /// Retaliation duties: the goods-weighted mean retaliatory duty on US exports to each bloc (decimal; stage 4-3, zero with
    /// `world_retaliation` off or no flagged shock).
    #[serde(default)]
    pub ret_tau: [f64; NB],
    /// Import-demand term of the bloc network, % (spec 2.3: 100 times the 3-month EWMA, clamped to +/-10; zero in the first
    /// twelve reported months).
    #[serde(default)]
    pub us_import_gap: [f64; NB],
    /// Diagnostics (additions to spec 1.6, for W2-15 and W2-16): z~_j of this month (percent) and the common component c_t.
    #[serde(default)]
    pub z_tilde: [f64; NB],
    #[serde(default)]
    pub c_t: f64,
    /// Stage 3 (spec 1.6): the market's monthly-average real price, % log deviation with the stochastic offset (p_bar), the
    /// real Brent level in 2024 USD, world supply and demand deviations (% of Q), OPEC+ output (mb/d), US tight-oil output
    /// deviation (mb/d), the inventory state (mb), and the OPEC+ spare capacity left (% of Q).
    #[serde(default)]
    pub p_bar_pct: f64,
    #[serde(default)]
    pub brent_usd: f64,
    #[serde(default)]
    pub oil_supply_pct: f64,
    #[serde(default)]
    pub oil_demand_pct: f64,
    #[serde(default)]
    pub opec_out_mbd: f64,
    #[serde(default)]
    pub us_prod_mbd: f64,
    #[serde(default)]
    pub stocks: f64,
    #[serde(default)]
    pub spare_pct: f64,
    /// Diagnostic (stage 3): the ABM oil log deviation the economy sees this month (spliced, offset removed), % (W3-14, W3-15).
    #[serde(default)]
    pub oil_dev_pct: f64,
    /// Diagnostic (stage-3 fix round, W3-18): the market's US-region demand deviation d_US of this month, % of Q.
    #[serde(default)]
    pub oil_us_demand_pct: f64,
    /// Stage 4-1 (spec 2.7): the financial factor F_t of this month, VIX points (`f_vix`; zero before the dollar state exists).
    #[serde(default)]
    pub f_vix: f64,
    /// Stage 4-1 (spec 2.6): Frisk_t - Frisk_b, VIX points (`f_risk`).
    #[serde(default)]
    pub f_risk: f64,
    /// Stage 4-1 (spec 2.6, all links): the broad dollar index relative to the base, 100 fx_t / fx_0 (`fx_broad`; 100 before the base).
    #[serde(default)]
    pub fx_broad: f64,
    /// Stage 4-1 (spec 2.6): 100 a_t, the common addend the blocs see (`a_dollar`).
    #[serde(default)]
    pub a_dollar: f64,
    /// Stage 4-1 (spec 2.6): the bilateral rates S_j = exp(lnS_j) of the dollar link, foreign currency per dollar (`s_bil`).
    #[serde(default)]
    pub s_bil: [f64; NB],
    /// List B (spec 2.6, rev 9): the conditional forecast z_fc,j of each bloc's z that the dollar link reads it against (`z_fc`); the
    /// base z with `world_dollar_neutral` off; zero before the base.
    #[serde(default)]
    pub z_fc: [f64; NB],
    /// Stage 4-2 (spec 2.7): FO^rw_t ($bn, real-world units), the executed flow of the month ($bn), the 10y term-premium terms in force
    /// in the month's curve (bp), and the cumulative net official sales at market value (model units).
    #[serde(default)]
    pub fo_bn: f64,
    #[serde(default)]
    pub fo_flow_bn: f64,
    #[serde(default)]
    pub tp_fo_bp: f64,
    #[serde(default)]
    pub tp_cred_bp: f64,
    #[serde(default)]
    pub fo_extra: f64,
    /// Stage 4-2 (spec 2.7): the six-month mean of F - Fbar that drives the VIX term of the official flow (VIX points).
    #[serde(default)]
    pub f_ring_mean: f64,
    /// Stage 4-2 (spec W4-20 and the leak check): RoW's bond market value and deposits as % of annualised model GDP, the official
    /// tranche as % of RoW's market value, and RoW's share of the face debt.
    #[serde(default)]
    pub row_mv_gdp_pct: f64,
    #[serde(default)]
    pub row_dep_gdp_pct: f64,
    #[serde(default)]
    pub fo_share_row_pct: f64,
    #[serde(default)]
    pub row_face_share_pct: f64,
    /// Stage 4-3 (spec 2.5, 2.8): US goods exports (Energy..Tech) by destination bloc, annualised $bn (the W4-24 statistic).
    #[serde(default)]
    pub exports_goods_bn: [f64; NB],
    /// Today's start (world-usnow in docs/CALIBRATION.md): US goods imports (Energy..Tech) by origin bloc, annualised $bn. Present only with the Today's trade
    /// reference (absent, so every other world response keeps its bytes).
    #[serde(default, skip_serializing_if = "all_zero")]
    pub imports_goods_bn: [f64; NB],
    /// Stage 4-3: the memo net international investment position, % of annualised model GDP; the current account (goods and services
    /// balance, interest paid to RoW and memo primary income), % of GDP; the current account by bloc, annualised $bn; the largest
    /// residual of the stock-flow identities of the month relative to monthly GDP.
    #[serde(default)]
    pub niip_pct_gdp: f64,
    #[serde(default)]
    pub ca_pct_gdp: f64,
    #[serde(default)]
    pub ca_bilateral_bn: [f64; NB],
    #[serde(default)]
    pub sfc_world_error: f64,
}
