//! Trade and prices (b8-world stage 2, spec 2.5): bilateral goods trade of the US with the twelve blocs by sector, origin
//! import prices under dominant-currency invoicing, origin shares, the partner-sector duty handed to b7, exports on foreign
//! income, and the US import-demand gap that feeds the bloc network (spec 2.3, D27).
//!
//! The state steps every reported month in `world::step`, after the bloc step (`pre_bloc` before it, `post_bloc` after it),
//! and settles the month's realised flows in `world::settle`. The ABM reads the outputs through hooks H6-H9 only when
//! `world::trade_active` holds (link on and reported month). With the link off the state is shadow: it reports bilateral
//! memo flows and changes nothing in the ABM. Nothing here touches a balance (spec 6.2).

use crate::common;
use crate::config::Config;
use crate::fiscal;
use crate::types::{role_mean, Sector, World, N_SECTORS};

use super::bloc::BlocState;
use super::params::{Trade, WorldParams};
use super::NB;

/// Sectors of the trade tables (model order: Energy, Food, ..., Construction).
pub const S: usize = N_SECTORS;
/// Energy's index: its origin prices carry no pass-through (r = 0, spec 2.5); its export competitor term is the world energy factor.
pub const ENERGY: usize = 0;
/// Cap of the total duty (decimal), spec 2.5.
pub const TAU_CAP: f64 = 5.0;
/// Monthly weight of the import-demand EWMA (spec 2.3: 3-month EWMA, 0.25).
pub const GAP_SPEED: f64 = 0.25;
/// Clamp of the import-demand term, percent (spec 2.3, +/-10).
pub const GAP_CLAMP: f64 = 10.0;
/// Reported months before the import-demand term acts: the months 0..11 use y_US only (spec 2.3).
pub const GAP_WARMUP: u32 = 12;
/// Stage 4-3 (spec 2.5, D9): ratchet speeds of the retaliation duty (`mu_up` 1, `mu_down` 0.1; research `recommended_rule.form`).
pub const RET_MU_UP: f64 = 1.0;
pub const RET_MU_DOWN: f64 = 0.1;
/// Stage 4-3: the first `RET_GOODS` sectors (Energy, Food, Durables, Nondurables, Tech) are the goods of the retaliation metrics.
pub const RET_GOODS: usize = 5;
/// Stage 4-3: longest lag of the retaliation rule in months (India, 12) plus one: the length of the history ring.
pub const RET_RING: usize = 13;

fn finite_or(x: f64, default: f64) -> f64 {
    if x.is_finite() {
        x
    } else {
        default
    }
}

/// Run-time coefficients of the trade block (the stage-2 `world_*` keys of section 1.5).
#[derive(Clone, Copy, Debug)]
pub struct TradePar {
    pub kappa_usd: f64,
    pub speed_usd: f64,
    pub kappa_nonusd: f64,
    pub speed_nonusd: f64,
    pub sigma_sr: f64,
    pub sigma_lr: f64,
    pub origin_speed: f64,
    pub eps_y: f64,
    pub price_sr: f64,
    pub price_lr: f64,
    pub fx_sr: f64,
    pub fx_lr: f64,
    pub export_speed: f64,
    /// `world_us_import_channel` (D27): the import-demand gap enters the bloc network in coupled mode.
    pub us_import_channel: bool,
    /// `world_import_gap_offset` (S2-R7): centring constants of the coupled term, pp per bloc (zeros when the key is empty).
    pub gap_offset: [f64; NB],
    /// `world_import_rel_drift` (5-T2, D62): decimal per year, 0 = off.
    pub rel_drift: f64,
}

impl TradePar {
    pub fn from_config(cfg: &Config) -> TradePar {
        TradePar {
            kappa_usd: cfg.world_dcp_kappa_usd,
            speed_usd: cfg.world_dcp_speed_usd,
            kappa_nonusd: cfg.world_dcp_kappa_nonusd,
            speed_nonusd: cfg.world_dcp_speed_nonusd,
            sigma_sr: cfg.world_origin_sigma_sr,
            sigma_lr: cfg.world_origin_sigma_lr,
            origin_speed: cfg.world_origin_speed,
            eps_y: cfg.world_export_income_elast,
            price_sr: cfg.world_export_price_sr,
            price_lr: cfg.world_export_price_lr,
            fx_sr: cfg.world_export_fx_sr,
            fx_lr: cfg.world_export_fx_lr,
            export_speed: cfg.world_export_speed,
            us_import_channel: cfg.world_us_import_channel,
            gap_offset: std::array::from_fn(|j| cfg.world_import_gap_offset.get(j).copied().filter(|x| x.is_finite()).unwrap_or(0.0)),
            rel_drift: if cfg.world_import_rel_drift.is_finite() { cfg.world_import_rel_drift } else { 0.0 },
        }
    }
}

/// One active bilateral duty component (`ShockState::world_shocks`, `Shock::Tariff`), in decimal. `partner` and `sector` are
/// bloc and sector indices, None for all.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TariffComp {
    pub partner: Option<usize>,
    pub sector: Option<usize>,
    pub tau: f64,
    pub retaliate: bool,
}

/// Constant tables derived from the embedded trade data (spec 2.5 notation).
#[derive(Clone, Debug)]
pub struct TradeTables {
    /// omega0[j][s]: bloc j's share of US imports of sector s (column shares of the 2024 imports, which sum to one).
    pub omega0: [[f64; S]; NB],
    /// x0[s][j]: US exports of sector s to bloc j over 2024 GDP (fraction).
    pub x0: [[f64; NB]; S],
    /// theta_j: USD-invoiced share of bloc j's exports to the US.
    pub theta: [f64; NB],
    /// m_jk[j][k] = bloc j's import share from bloc k (research `import_shares_2024[k][j]`); the diagonal is not used.
    pub m_jk: [[f64; NB]; NB],
    /// Normalised H10 weights (`wH10`), summing to one.
    pub dollar_w: [f64; NB],
    /// Sector totals of the 2024 imports (base weights of the origin sums before the first settled month).
    pub m_base: [f64; S],
}

impl TradeTables {
    pub fn from_params(t: &Trade) -> TradeTables {
        let mut omega0 = [[0.0; S]; NB];
        for s in 0..S {
            let col: f64 = (0..NB).map(|j| t.imports_base[j][s]).sum();
            for j in 0..NB {
                omega0[j][s] = if col > 0.0 { t.imports_base[j][s] / col } else { 1.0 / NB as f64 };
            }
        }
        let mut x0 = [[0.0; NB]; S];
        for s in 0..S {
            for j in 0..NB {
                x0[s][j] = t.exports_base[j][s] / t.gdp_2024_bn;
            }
        }
        let mut m_jk = [[0.0; NB]; NB];
        for j in 0..NB {
            for k in 0..NB {
                m_jk[j][k] = t.import_shares[k][j];
            }
        }
        let theta: [f64; NB] = std::array::from_fn(|j| t.theta[j]);
        let dollar_w: [f64; NB] = std::array::from_fn(|j| t.dollar_weights[j]);
        let m_base: [f64; S] = std::array::from_fn(|s| (0..NB).map(|j| t.imports_base[j][s]).sum());
        TradeTables { omega0, x0, theta, m_jk, dollar_w, m_base }
    }
}

/// The level of the tradables price index PT (role mean of the last cleared sector prices, spec 2.1).
pub fn pt_level(w: &World) -> f64 {
    role_mean(&w.cfg.tradable_weight, |s| common::sector_price(w, Sector::from_idx(s)))
}

/// Bases of the trade block, captured at the end of the last burn-in month (spec D36, 1.6). Stage-2 quantities only; the
/// stage-1 bases are `WorldBlock::bases`. The bases are the ABM's own values in that month, so no ABM state is overwritten
/// at the first reported month and a dollar shock at reported month 0 moves the real dollar against them.
#[derive(Clone, Debug)]
pub struct TradeBase {
    /// Exchange rate and world price at the base (`w.row.fx`, `common::world_price`).
    pub fx0: f64,
    pub p0: f64,
    /// PT at the base.
    pub pt0: f64,
    /// ln(sector price / PT) at the base, by sector (the bracket of the export price term is zero at reported month 0).
    pub base_s: [f64; S],
    /// The ABM's legacy import price level in the base month (`import_rel` of `goods::clear`): the level the block's import
    /// prices start from, so the terms ratio and the CPI import component continue from the burn-in.
    pub imp_level: f64,
    /// Export level scale by sector: the ABM's legacy exports of the base month over the block's exports at zero states (the
    /// base export formula with the bloc outputs of that month). One when the ABM has not cleared a month (tests).
    pub kappa: [f64; S],
    /// The cross-section z_j - sum_k wH10_k z_k of the bloc outputs at the base month: the dollar-invoicing and export FX terms
    /// move with its change since the base (the base is the reference, so the block's prices and exports start at the base).
    pub z_base: [f64; NB],
    /// Stage 4-1: the raw bloc outputs z_j of the base month (`hist.z[0]`). In dollar mode the pass-through reads
    /// z~_j = z_j + 100 a_j (spec 2.5) and moves with its change since this level.
    pub z_level: [f64; NB],
    /// ln of the world energy factor at the base (Energy's export competitor term is read as its change since the base).
    pub world_f0: f64,
    /// Import and export totals of the base month, in model units (volume indices are relative to them).
    pub imp0: f64,
    pub exp0: f64,
}

/// Half-life of the slow benchmark of ln(Q/Y) in the import-demand term, months (spec 2.3, S2-R5).
pub const BENCH_HALF_LIFE: f64 = 60.0;

/// Monthly weight of the slow benchmark: 1 - 2^(-1/half-life).
fn bench_speed() -> f64 {
    1.0 - 0.5f64.powf(1.0 / BENCH_HALF_LIFE)
}

/// The bloc's output gap as the export formula reads it: the mean of its last three outputs (y of t-1..t-3), decimal.
fn yhat_of(blocs: &[BlocState; NB], j: usize) -> f64 {
    (blocs[j].hist.y[1] + blocs[j].hist.y[2] + blocs[j].hist.y[3]) / 3.0 / 100.0
}

/// Stage 4-3 (spec 2.5, D9): the retaliation state. Zero (and the factor exactly 1.0) until a `Tariff` flagged `retaliate` is active.
#[derive(Clone, Debug)]
pub struct RetState {
    pub on: bool,
    pub eps_sr: f64,
    pub eps_lr: f64,
    pub speed: f64,
    pub kappa: [f64; NB],
    pub cap: [f64; NB],
    pub lag: [usize; NB],
    pub w_s: [f64; S],
    /// Import values of the 2024 base by origin and sector (fixed weights of the average duty taubar_j).
    pub w0: [[f64; S]; NB],
    /// History of the average US duty on bloc j, [0] = this month.
    pub hist: [[f64; RET_RING]; NB],
    /// tau*_{j,s}, the lagged price state zr_{j,s} and the export factor R_{j,s} of this month.
    pub tau_star: [[f64; S]; NB],
    pub zr: [[f64; S]; NB],
    pub r: [[f64; S]; NB],
    /// Today's start (world-usnow in docs/CALIBRATION.md): the retaliatory duty already in place at the reference, by bloc and goods group (decimal), and its log. Zero
    /// without a reference, which is the merged rule exactly (adding zero and subtracting zero change no bit).
    pub tau_ref: [[f64; S]; NB],
    pub x_ref: [[f64; S]; NB],
}

impl RetState {
    pub fn new(cfg: &Config, wp: &WorldParams) -> RetState {
        let t = &wp.trade;
        let rt = &t.retaliation;
        let ok = rt.kappa.len() == NB && rt.lag.len() == NB && rt.cap.len() == NB && rt.w_s.len() == S;
        RetState {
            on: cfg.world_retaliation && ok,
            eps_sr: cfg.world_ret_eps_sr,
            eps_lr: cfg.world_ret_eps_lr,
            speed: cfg.world_ret_speed,
            kappa: std::array::from_fn(|j| if ok { rt.kappa[j] } else { 0.0 }),
            cap: std::array::from_fn(|j| if ok { rt.cap[j] } else { 0.0 }),
            lag: std::array::from_fn(|j| if ok { rt.lag[j].min(RET_RING - 1) } else { 0 }),
            w_s: std::array::from_fn(|s| if ok { rt.w_s[s] } else { 0.0 }),
            w0: std::array::from_fn(|j| std::array::from_fn(|s| t.imports_base[j][s])),
            hist: [[0.0; RET_RING]; NB],
            tau_star: [[0.0; S]; NB],
            zr: [[0.0; S]; NB],
            r: [[1.0; S]; NB],
            tau_ref: [[0.0; S]; NB],
            x_ref: [[0.0; S]; NB],
        }
    }

    /// The average US duty on bloc j over the retaliating components, weighted by the 2024 import values (fixed weights).
    pub fn taubar(&self, tariffs: &[TariffComp], j: usize) -> f64 {
        let (mut num, mut den) = (0.0, 0.0);
        for s in 0..S {
            let add: f64 = tariffs
                .iter()
                .filter(|c| c.retaliate && c.partner.map_or(true, |p| p == j) && c.sector.map_or(true, |x| x == s))
                .map(|c| c.tau)
                .sum();
            num += self.w0[j][s] * add.min(TAU_CAP);
            den += self.w0[j][s];
        }
        if den > 0.0 { num / den } else { 0.0 }
    }

    /// One reported month: push taubar, read it `lag` months back, target, ratchet, price state and factor (spec 2.5).
    pub fn step(&mut self, tariffs: &[TariffComp]) {
        for j in 0..NB {
            let tb = self.taubar(tariffs, j);
            for k in (1..RET_RING).rev() {
                self.hist[j][k] = self.hist[j][k - 1];
            }
            self.hist[j][0] = tb;
            let lagged = self.hist[j][self.lag[j]];
            for s in 0..S {
                // Today's start: the target is the duty already in place plus the rule's response to the new US duty; with no reference both are zero.
                let tgt = (self.tau_ref[j][s] + self.kappa[j] * self.w_s[s] * lagged).min(self.cap[j]);
                let mu = if tgt >= self.tau_star[j][s] { RET_MU_UP } else { RET_MU_DOWN };
                self.tau_star[j][s] += mu * (tgt - self.tau_star[j][s]);
                let x = (1.0 + self.tau_star[j][s]).ln() - self.x_ref[j][s];
                self.zr[j][s] += self.speed * (x - self.zr[j][s]);
                self.r[j][s] = (-(self.eps_sr * x + (self.eps_lr - self.eps_sr) * self.zr[j][s])).exp();
            }
        }
    }
}

/// Today's start (world-usnow in docs/CALIBRATION.md): the US duty already in place by origin and goods group (decimal, total), and the set-level duty
/// (`ImportTariff`) at the moment it was installed. A change of the set level since then moves every cell by the same points.
#[derive(Clone, Copy, Debug)]
pub struct UsNowRef {
    pub tau_ref: [[f64; S]; NB],
    pub lvl_ref: f64,
}

/// The trade state (spec 1.2 `TradeState`).
#[derive(Clone, Debug)]
pub struct TradeState {
    pub par: TradePar,
    pub tab: TradeTables,
    pub base: Option<TradeBase>,
    /// Phi^d_j and Phi^nd_j (the dominant-currency and non-dollar import price states, log points x 100).
    pub phi_d: [f64; NB],
    pub phi_nd: [f64; NB],
    /// Lagged origin price states zw_{j,s} (spec 2.5, origin shares).
    pub zw: [[f64; S]; NB],
    /// Importer depreciation state uf_j (export exchange-rate term).
    pub uf: [f64; NB],
    /// Competitor-price state up_{s,j} (export price term).
    pub up: [[f64; NB]; S],
    /// Bloc j's common-free real exchange rate z~_j (this month) and the common component c_t (diagnostics, spec 2.5).
    pub z_tilde: [f64; NB],
    pub c_t: f64,
    /// Origin log price r_j (this month; Energy's r is zero).
    pub r: [f64; NB],
    /// Origin shares omega_{j,s} (this month).
    pub omega: [[f64; S]; NB],
    /// Exports to each bloc by sector X_{s,j} (this month, model units).
    pub x_exp: [[f64; NB]; S],
    /// Active bilateral duty components (`Shock::Tariff`), in activation order.
    pub tariffs: Vec<TariffComp>,
    /// Stage 4-3: the retaliation state (spec 2.5).
    pub ret: RetState,
    /// Today's start (world-usnow in docs/CALIBRATION.md): the duty reference by origin and goods group, set once by `rebase_today`.
    pub usnow: Option<UsNowRef>,
    /// Customs imports by sector of the previous month (m_s of spec 2.5; the 2024 base before the first settled month).
    pub m_prev: [f64; S],
    /// Exports by sector of the previous month (model units).
    pub exp_prev: [f64; S],
    /// Real imports from each bloc of the previous month, Q_{i,t-1}, relative to the base (zero before the base).
    pub q_prev: [f64; NB],
    /// Import-demand EWMA of the deviation of ln(Q/Y) from its slow benchmark (spec 2.3) and its last three terms (percent;
    /// month t-1 first). The benchmark is the mean of the first `GAP_WARMUP` observations of each bloc, then an EWMA with the
    /// half-life `BENCH_HALF_LIFE` (S2-R5): `bench_n` counts the warm-up observations, `bench_sum` accumulates them.
    pub gap_ewma: [f64; NB],
    pub gap_ring: [[f64; 3]; NB],
    pub bench_n: [u32; NB],
    pub bench_sum: [f64; NB],
    pub bench: [f64; NB],
    /// Reported months stepped since the base was captured.
    pub since_base: u32,
    /// PT_{t-1} over the base PT of this month's import prices (`post_bloc`); the real imports are deflated by it in `settle`
    /// too (F4). One before the base.
    pub pt_ratio: f64,
    /// This month's customs imports from each bloc and exports to each bloc (settled, model units).
    pub imp_bloc: [f64; NB],
    /// Today's start (world-usnow in docs/CALIBRATION.md): this month's goods imports (Energy..Tech) from each bloc, settled, model units.
    pub imp_goods: [f64; NB],
    pub exp_bloc: [f64; NB],
    /// Stage 4-3: this month's goods exports (sectors Energy..Tech) to each bloc, settled, model units.
    pub exp_goods: [f64; NB],
    /// Import and export volume indices (100 at the base) of the settled month.
    pub imp_vol_idx: f64,
    pub exp_vol_idx: f64,
}

/// This month's values the ABM reads (spec 1.2 `WorldOut` extension, hooks H6-H8) and the memo values.
#[derive(Clone, Debug)]
pub struct TradeOut {
    /// Import price relative to the base by sector, sector-specific origin terms (`imp_rel_s`, H6).
    pub imp_rel: [f64; S],
    /// CPI-weighted import price (`imp_rel_cpi`, H6).
    pub imp_rel_cpi: f64,
    /// Effective duty per channel (decimal, H8).
    pub tau_eff_hh: f64,
    pub tau_eff_firm: f64,
    /// Exports by sector, nominal model units per month (H7).
    pub exports: [f64; S],
    /// Memo: duty by origin, decimal (`tau_by_origin_j`).
    pub tau_by_origin: [f64; NB],
    /// Stage 4-3: the average retaliatory duty on US goods exports to each bloc (decimal, weights x0, sectors Energy..Tech).
    pub ret_tau: [f64; NB],
}

impl Default for TradeOut {
    fn default() -> Self {
        TradeOut { imp_rel: [1.0; S], imp_rel_cpi: 1.0, tau_eff_hh: 0.0, tau_eff_firm: 0.0, exports: [0.0; S], tau_by_origin: [0.0; NB], ret_tau: [0.0; NB] }
    }
}

impl TradeState {
    pub fn new(cfg: &Config, wp: &WorldParams) -> TradeState {
        let tab = TradeTables::from_params(&wp.trade);
        TradeState {
            par: TradePar::from_config(cfg),
            tab: tab.clone(),
            base: None,
            phi_d: [0.0; NB],
            phi_nd: [0.0; NB],
            zw: [[0.0; S]; NB],
            uf: [0.0; NB],
            up: [[0.0; NB]; S],
            z_tilde: [0.0; NB],
            c_t: 0.0,
            r: [0.0; NB],
            omega: tab.omega0,
            x_exp: [[0.0; NB]; S],
            tariffs: Vec::new(),
            ret: RetState::new(cfg, wp),
            usnow: None,
            m_prev: tab.m_base,
            exp_prev: [0.0; S],
            q_prev: [0.0; NB],
            gap_ewma: [0.0; NB],
            gap_ring: [[0.0; 3]; NB],
            bench_n: [0; NB],
            bench_sum: [0.0; NB],
            bench: [0.0; NB],
            since_base: 0,
            pt_ratio: 1.0,
            imp_bloc: [0.0; NB],
            imp_goods: [0.0; NB],
            exp_bloc: [0.0; NB],
            exp_goods: [0.0; NB],
            imp_vol_idx: 100.0,
            exp_vol_idx: 100.0,
        }
    }

    /// Captures the trade bases from the ABM's values of the base month: the end of the last burn-in month (`WorldBlock::settle`,
    /// after the month's settlement, so `m_prev` and `exp_prev` hold that month), or the first step of a world without burn-in.
    /// The legacy import price and exports of the month are the ABM's stored clear outputs (`goods.import_rel`,
    /// `orders.exports`); before the first clear (tests) the import price falls back to `goods::import_pass_through` (one at
    /// the neutral base) and kappa to one. `blocs` gives the bloc outputs of the base month for the base export formula.
    pub fn capture(&mut self, w: &World, blocs: &[BlocState; NB]) {
        let fx0 = finite_or(w.row.fx, 1.0);
        let p0 = common::world_price(w);
        let pt0 = pt_level(w);
        let base_s: [f64; S] = std::array::from_fn(|s| {
            let ps = common::sector_price(w, Sector::from_idx(s));
            if pt0 > 0.0 && ps > 0.0 { (ps / pt0).ln() } else { 0.0 }
        });
        let imp_level = w
            .goods
            .import_rel
            .filter(|x| x.is_finite() && *x > 0.0)
            .unwrap_or_else(|| crate::goods::import_pass_through(w, &w.goods).0);
        let trend = fiscal::trend_gdp_month(w);
        let kappa: [f64; S] = std::array::from_fn(|s| {
            let leg = w.orders.exports[s];
            let blk: f64 = (0..NB).map(|j| self.tab.x0[s][j] * trend * (self.par.eps_y * yhat_of(blocs, j)).exp()).sum();
            if leg.is_finite() && leg > 0.0 && blk.is_finite() && blk > 0.0 { leg / blk } else { 1.0 }
        });
        let z: [f64; NB] = std::array::from_fn(|j| blocs[j].hist.z[0]);
        let zbar: f64 = (0..NB).map(|k| self.tab.dollar_w[k] * z[k]).sum();
        let z_base: [f64; NB] = std::array::from_fn(|j| z[j] - zbar);
        let imp0: f64 = self.m_prev.iter().sum();
        let exp0: f64 = self.exp_prev.iter().sum();
        let world_f0 = common::world_sector_factor(w, ENERGY).ln();
        let world_f0 = if world_f0.is_finite() { world_f0 } else { 0.0 };
        self.base = Some(TradeBase { fx0, p0, pt0, base_s, imp_level, kappa, z_base, z_level: z, world_f0, imp0, exp0 });
        self.gap_ewma = [0.0; NB];
        self.gap_ring = [[0.0; 3]; NB];
        self.bench_n = [0; NB];
        self.bench_sum = [0.0; NB];
        self.bench = [0.0; NB];
        self.since_base = 0;
        self.pt_ratio = 1.0;
    }

    /// Today's start (docs/CALIBRATION.md, section world-usnow): install the dated reference at the end of the steered window. The origin shares and the export
    /// destination shares become today's (each goods group keeps its total), the duty by origin and goods group and the partners' retaliatory duties
    /// become the reference every later response is measured against, and the share, price and retaliation states restart at zero, so a world with no
    /// change holds today's state. `lvl` is the set-level duty (`ImportTariff`) in force. Services groups keep the 2024 shares.
    pub fn rebase_today(&mut self, v: &super::usnow::RefVintage, lvl: f64) {
        // The import-demand term compares ln(Q_j / Y), the real imports from bloc j, with a slow benchmark learned since the base (spec 2.3, S2-R5).
        // Re-basing the shares changes Q_j at once, with no change in demand, so the log of the change in each bloc's import weight moves the
        // benchmark (and its warm-up sum) and last month's quantity by the same amount: the term is continuous at the reference. Without this
        // the blocs read the re-basing as a collapse of US demand for the origins whose share fell (Canada, in probe 2).
        let weight = |om: &[[f64; S]; NB], j: usize| -> f64 { (0..S).map(|s| om[j][s] * self.m_prev[s]).sum() };
        let old_w: [f64; NB] = std::array::from_fn(|j| weight(&self.omega, j));
        let mut tau_ref = [[0.0; S]; NB];
        for s in 0..RET_GOODS {
            // The shares are normalised here, so each goods group's origin shares sum to one and its export total is kept exactly.
            let m: f64 = (0..NB).map(|j| v.omega[j][s]).sum();
            let x: f64 = (0..NB).map(|j| v.export_dest[j][s]).sum();
            let tot: f64 = (0..NB).map(|j| self.tab.x0[s][j]).sum();
            for j in 0..NB {
                tau_ref[j][s] = v.duty[j][s];
                self.tab.omega0[j][s] = v.omega[j][s] / m;
                self.tab.x0[s][j] = tot * v.export_dest[j][s] / x;
            }
        }
        self.omega = self.tab.omega0;
        self.zw = [[0.0; S]; NB];
        for j in 0..NB {
            let new_w = weight(&self.omega, j);
            if old_w[j] > 0.0 && new_w > 0.0 {
                let ratio = new_w / old_w[j];
                let d = ratio.ln();
                self.q_prev[j] *= ratio;
                self.bench[j] += d;
                self.bench_sum[j] += f64::from(self.bench_n[j]) * d;
            }
        }
        self.usnow = Some(UsNowRef { tau_ref, lvl_ref: lvl });
        // The partners' retaliatory duty: the bloc's average (goods-weighted by the new export shares) spread over the goods groups by the rule's
        // sector weights, so the average equals the measured one. Exports are measured against it: R = 1 at the reference.
        let rt = &mut self.ret;
        for j in 0..NB {
            let den: f64 = (0..RET_GOODS).map(|s| self.tab.x0[s][j]).sum();
            let wbar: f64 = if den > 0.0 { (0..RET_GOODS).map(|s| self.tab.x0[s][j] * rt.w_s[s]).sum::<f64>() / den } else { 0.0 };
            for s in 0..RET_GOODS {
                let t = if wbar > 0.0 { (v.ret[j] * rt.w_s[s] / wbar).min(rt.cap[j].max(v.ret[j])) } else { 0.0 };
                rt.tau_ref[j][s] = t;
                rt.x_ref[j][s] = (1.0 + t).ln();
                rt.tau_star[j][s] = t;
                rt.zr[j][s] = 0.0;
                rt.r[j][s] = 1.0;
            }
        }
    }

    /// Before the bloc step of a reported month: the import-demand term of month t-1 enters the three-month ring
    /// (spec 2.3). The ring is read by `world::core` only in coupled mode. No-op before the base.
    pub fn pre_bloc(&mut self, w: &World) {
        if self.base.is_none() {
            return;
        }
        // The EWMA of month t-1 needs Q_{t-1} (settled last month) and Y_{t-1} (the last metrics record). The level x = ln(Q/Y)
        // is compared with a slow benchmark (S2-R5): the mean of the first GAP_WARMUP observations, then an EWMA with the
        // half-life BENCH_HALF_LIFE. The deviation enters the three-month EWMA. No observation before the first settled month.
        let y_prev = w.last().map(|m| m.gdp_real_bn_ar).filter(|y| y.is_finite() && *y > 0.0).unwrap_or(1.0);
        let alpha = bench_speed();
        for i in 0..NB {
            if self.q_prev[i] > 0.0 {
                let x = (self.q_prev[i] / y_prev).ln();
                if !x.is_finite() {
                    continue;
                }
                if self.bench_n[i] < GAP_WARMUP {
                    self.bench_sum[i] += x;
                    self.bench_n[i] += 1;
                    if self.bench_n[i] == GAP_WARMUP {
                        self.bench[i] = self.bench_sum[i] / f64::from(GAP_WARMUP);
                    }
                } else {
                    let dev = x - self.bench[i];
                    self.gap_ewma[i] += GAP_SPEED * (dev - self.gap_ewma[i]);
                    self.bench[i] += alpha * (x - self.bench[i]);
                }
            }
        }
        // Months 0..11 of the reported run use y_US only: the term of month t-1 applies from reported month 13 on.
        let active = self.since_base >= GAP_WARMUP + 1;
        for i in 0..NB {
            let term = if active { (100.0 * self.gap_ewma[i]).clamp(-GAP_CLAMP, GAP_CLAMP) } else { 0.0 };
            let term = if term.is_finite() { term } else { 0.0 };
            let term = if active { term - self.par.gap_offset[i] } else { term };
            self.gap_ring[i] = [term, self.gap_ring[i][0], self.gap_ring[i][1]];
        }
    }

    /// The month's trade computations after the bloc step (spec 2.5): the real exchange rate and dominant-currency prices,
    /// origin shares, the import price indices, the duties per channel and the exports. Reads this month's bloc outputs
    /// (`hist.z[0]`, and `hist.y[1..=3]` = y of t-1..t-3, the bloc step having pushed y of t).
    ///
    /// Stage 4-1: `dollar` = the bilateral addends a_{j,t} of the dollar link (`Some` when the link acts this month). Then
    /// z~_j = z_j + 100 a_j and the change is measured against `z_level`; `None` is the merged code, unchanged.
    #[inline(never)]
    pub fn post_bloc(&mut self, w: &World, blocs: &[BlocState; NB], dollar: Option<&[f64; NB]>) -> TradeOut {
        self.post_bloc_ref(w, blocs, dollar, None)
    }

    /// `post_bloc` with the list-B reference (spec 2.5, rev 9): `zref` = the z each bloc is read against in dollar mode (its conditional
    /// forecast, `DollarState::z_ref`); then `zrel_j = z_j - zref_j + 100 a_j`. `None`: the 4-1 form, `z~_j - z_level_j`.
    #[inline(never)]
    pub fn post_bloc_ref(&mut self, w: &World, blocs: &[BlocState; NB], dollar: Option<&[f64; NB]>, zref: Option<&[f64; NB]>) -> TradeOut {
        let t_level = finite_or(w.shocks.tariff, 0.0).max(0.0);
        let Some(b) = self.base.clone() else {
            // Before the base (the burn-in): the states wait for the base; they start at zero there (spec 1.6, D36).
            return TradeOut { tau_eff_hh: t_level, tau_eff_firm: t_level, ..TradeOut::default() };
        };
        let fx = finite_or(w.row.fx, b.fx0);
        let p_t = common::world_price(w);
        let pt = pt_level(w);
        let pt_ratio = if b.pt0 > 0.0 && pt > 0.0 { pt / b.pt0 } else { 1.0 };
        self.pt_ratio = pt_ratio;
        // Common component c_t: the ABM's own real dollar relative to the base (spec 2.5). Before the base c_t is zero.
        let c_t = 100.0 * ((fx / b.fx0).ln() + (pt / b.pt0).ln() - (p_t / b.p0).ln());
        let c_t = if c_t.is_finite() { c_t } else { 0.0 };
        self.c_t = c_t;
        // Cross-section: z~_j = c_t + (z_j - sum_k wH10_k z_k), with the normalised weights.
        let z: [f64; NB] = std::array::from_fn(|j| blocs[j].hist.z[0]);
        let zbar: f64 = (0..NB).map(|k| self.tab.dollar_w[k] * z[k]).sum();
        self.z_tilde = match dollar {
            Some(a) => std::array::from_fn(|j| z[j] + 100.0 * a[j]),
            None => std::array::from_fn(|j| c_t + (z[j] - zbar)),
        };
        // Dominant-currency and non-dollar import price states (spec 2.5; speeds and long-run responses from the config).
        // The price and export terms read the change of z~ since the base (`z_base`): at the base they are zero, so the import
        // prices and exports start from the ABM's base-month levels (the continuity rule of D36).
        let zrel: [f64; NB] = match dollar {
            Some(a) => match zref {
                Some(zr) => std::array::from_fn(|j| (z[j] - zr[j]) + 100.0 * a[j]),
                None => std::array::from_fn(|j| self.z_tilde[j] - b.z_level[j]),
            },
            None => std::array::from_fn(|j| self.z_tilde[j] - b.z_base[j]),
        };
        for j in 0..NB {
            self.phi_d[j] += self.par.speed_usd * (self.par.kappa_usd * zrel[j] - self.phi_d[j]);
            self.phi_nd[j] += self.par.speed_nonusd * (self.par.kappa_nonusd * zrel[j] - self.phi_nd[j]);
            let th = self.tab.theta[j];
            self.r[j] = -(th * self.phi_d[j] + (1.0 - th) * self.phi_nd[j]) / 100.0;
        }
        // Duties by origin and sector: the set-level level T plus the bilateral components, capped (D34, decimal).
        // 5-T2 (D63): a component with no partner and no sector is a uniform duty and is folded into the level, so `Tariff{null, null, p}` and
        // `ImportTariff{p}` give the same floating-point value through every channel (bit-identical runs); with no such component
        // `t_uni` is 0.0 and `t_level + 0.0` is `t_level`.
        let t_uni: f64 = self.tariffs.iter().filter(|c| c.partner.is_none() && c.sector.is_none()).map(|c| c.tau).sum();
        let t_level = t_level + t_uni;
        let mut tau = [[0.0; S]; NB];
        for j in 0..NB {
            for s in 0..S {
                let add: f64 = self
                    .tariffs
                    .iter()
                    .filter(|c| (c.partner.is_some() || c.sector.is_some()) && c.partner.map_or(true, |p| p == j) && c.sector.map_or(true, |x| x == s))
                    .map(|c| c.tau)
                    .sum();
                tau[j][s] = (t_level + add).min(TAU_CAP);
            }
        }
        // Duty-inclusive log prices q_{j,s} (energy origins carry no pass-through) and the origin shares.
        // Today's start: the share response is to the duty measured against the reference (ln of the ratio of the new duty to the one in
        // place), so with no change it is zero; without a reference the expression is the merged one.
        let mut q = [[0.0; S]; NB];
        let mut tau_memo = tau;
        for j in 0..NB {
            for s in 0..S {
                let r_js = if s == ENERGY { 0.0 } else { self.r[j] };
                q[j][s] = match self.usnow {
                    None => r_js + (1.0 + tau[j][s]).ln(),
                    Some(u) => {
                        let add = tau[j][s] - t_level;
                        let now = (u.tau_ref[j][s] + (t_level - u.lvl_ref) + add).max(0.0).min(TAU_CAP);
                        tau_memo[j][s] = now;
                        r_js + (1.0 + now).ln() - (1.0 + u.tau_ref[j][s]).ln()
                    }
                };
            }
        }
        for s in 0..S {
            let qbar: f64 = (0..NB).map(|k| self.tab.omega0[k][s] * q[k][s]).sum();
            let mut e = [0.0; NB];
            for j in 0..NB {
                let x = q[j][s] - qbar;
                self.zw[j][s] += self.par.origin_speed * (x - self.zw[j][s]);
                e[j] = (-(self.par.sigma_sr * x + (self.par.sigma_lr - self.par.sigma_sr) * self.zw[j][s])).exp();
            }
            let den: f64 = (0..NB).map(|k| self.tab.omega0[k][s] * e[k]).sum();
            for j in 0..NB {
                self.omega[j][s] = self.tab.omega0[j][s] * e[j] / den;
            }
        }
        // Import price relative to the ABM's t=0 base by sector: the base-month legacy level (`imp_level`), times PT of t-1
        // over the base PT, times the origin factor; CPI-weighted index.
        // 5-T2 (D62): the baseline drift of the import price against PT (zero at the base month: continuity), 1.0 when the key is 0.
        let drift = if self.par.rel_drift != 0.0 { (-self.par.rel_drift * f64::from(self.since_base) / 12.0).exp() } else { 1.0 };
        let mut imp_rel = [0.0; S];
        for s in 0..S {
            let rbar: f64 = if s == ENERGY { 0.0 } else { (0..NB).map(|j| self.omega[j][s] * self.r[j]).sum() };
            imp_rel[s] = b.imp_level * pt_ratio * rbar.exp() * drift;
        }
        let cpi_w = w.cfg.cpi_tradable_weight;
        let imp_share = w.cfg.import_share;
        let num = role_mean(&cpi_w, |s| imp_share[s] * imp_rel[s]);
        let den = role_mean(&cpi_w, |s| imp_share[s]);
        let imp_rel_cpi = if den > 0.0 && num.is_finite() { num / den } else { 1.0 };
        // Duties per channel: the household channel over the CPI-tradable sectors, the firm channel over the tradable sectors,
        // weighted by last month's imports by sector (spec 2.5).
        let cov_hh: [bool; S] = std::array::from_fn(|s| w.cfg.cpi_tradable_weight[s] != 0.0);
        let cov_firm: [bool; S] = std::array::from_fn(|s| w.cfg.tradable_weight[s] != 0.0);
        let tau_eff_hh = self.tau_channel(&cov_hh, &tau, t_level);
        let tau_eff_firm = self.tau_channel(&cov_firm, &tau, t_level);
        // Exports on foreign income, the importer exchange rate and the competitor-price terms (spec 2.5, R = 1 in stage 2).
        let trend = fiscal::trend_gdp_month(w);
        let world_f = common::world_sector_factor(w, ENERGY);
        let mut exports = [0.0; S];
        let yhat: [f64; NB] = std::array::from_fn(|j| yhat_of(blocs, j));
        let mut lnfx = [0.0; NB];
        for j in 0..NB {
            self.uf[j] += self.par.export_speed * (zrel[j] / 100.0 - self.uf[j]);
            lnfx[j] = -(self.par.fx_sr * zrel[j] / 100.0 + (self.par.fx_lr - self.par.fx_sr) * self.uf[j]);
        }
        // Competitor price of bloc j: the import-share-weighted origin prices of the other blocs (no US term).
        let rc: [f64; NB] = std::array::from_fn(|j| {
            let mut num = 0.0;
            let mut den = 0.0;
            for k in 0..NB {
                if k != j {
                    num += self.tab.m_jk[j][k] * self.r[k];
                    den += self.tab.m_jk[j][k];
                }
            }
            if den > 0.0 { num / den } else { 0.0 }
        });
        if self.ret.on {
            self.ret.step(&self.tariffs);
        }
        for s in 0..S {
            let ps = common::sector_price(w, Sector::from_idx(s));
            let dev = if pt > 0.0 && ps > 0.0 { (ps / pt).ln() - b.base_s[s] } else { 0.0 };
            for j in 0..NB {
                let xrp = if s == ENERGY { (world_f.ln() - b.world_f0) - dev } else { rc[j] - dev };
                let xrp = if xrp.is_finite() { xrp } else { 0.0 };
                self.up[s][j] += self.par.export_speed * (xrp - self.up[s][j]);
                let lnrp = self.par.price_sr * xrp + (self.par.price_lr - self.par.price_sr) * self.up[s][j];
                let x = b.kappa[s] * self.tab.x0[s][j] * trend * (self.par.eps_y * yhat[j] + lnfx[j] + lnrp).exp();
                let x = if self.ret.on { x * self.ret.r[j][s] } else { x };
                self.x_exp[s][j] = if x.is_finite() && x > 0.0 { x } else { 0.0 };
                exports[s] += self.x_exp[s][j];
            }
        }
        let tau_by_origin: [f64; NB] = std::array::from_fn(|j| {
            let num: f64 = (0..S).map(|s| self.omega[j][s] * self.m_prev[s] * (tau[j][s] - t_level)).sum();
            let den: f64 = (0..S).map(|s| self.omega[j][s] * self.m_prev[s]).sum();
            if self.usnow.is_some() {
                // Today's start: the memo is the duty in place by origin (the reference plus the changes since), weighted like the merged memo.
                let tot: f64 = (0..RET_GOODS).map(|s| self.omega[j][s] * self.m_prev[s] * tau_memo[j][s]).sum();
                let dg: f64 = (0..RET_GOODS).map(|s| self.omega[j][s] * self.m_prev[s]).sum();
                return if dg > 0.0 { tot / dg } else { t_level };
            }
            if den > 0.0 { t_level + num / den } else { t_level }
        });
        let ret_tau: [f64; NB] = std::array::from_fn(|j| {
            let den: f64 = (0..RET_GOODS).map(|s| self.tab.x0[s][j]).sum();
            let num: f64 = (0..RET_GOODS).map(|s| self.tab.x0[s][j] * self.ret.tau_star[j][s]).sum();
            if self.ret.on && den > 0.0 { num / den } else { 0.0 }
        });
        self.since_base += 1;
        TradeOut { imp_rel, imp_rel_cpi, tau_eff_hh, tau_eff_firm, exports, tau_by_origin, ret_tau }
    }

    /// tau_eff^c = sum over covered sectors of m_s sum_j omega_{j,s} tau_{j,s} / sum of m_s over the covered sectors, written
    /// as T + the weighted bilateral excess, so a uniform tariff returns T exactly.
    fn tau_channel(&self, cov: &[bool; S], tau: &[[f64; S]; NB], t_level: f64) -> f64 {
        let mut num = 0.0;
        let mut den = 0.0;
        for s in 0..S {
            if cov[s] {
                den += self.m_prev[s];
                for j in 0..NB {
                    num += self.m_prev[s] * self.omega[j][s] * (tau[j][s] - t_level);
                }
            }
        }
        if den > 0.0 {
            t_level + num / den
        } else {
            t_level
        }
    }

    /// Settles this month's realised flows (after `goods::clear`): customs imports by origin, exports by destination,
    /// the real import-demand terms of the next month and the volume indices. Writes no balance (spec 6.2).
    pub fn settle(&mut self, w: &World) {
        let imp_s = w.flows.imports_by_sector;
        let exp_s = w.flows.exports_by_sector;
        for j in 0..NB {
            self.imp_bloc[j] = (0..S).map(|s| self.omega[j][s] * imp_s[s]).sum();
        }
        if self.usnow.is_some() {
            for j in 0..NB {
                self.imp_goods[j] = (0..RET_GOODS).map(|s| self.omega[j][s] * imp_s[s]).sum();
            }
        }
        let mut exp_bloc = [0.0; NB];
        let mut exp_goods = [0.0; NB];
        for s in 0..S {
            let tot: f64 = (0..NB).map(|j| self.x_exp[s][j]).sum();
            if tot > 0.0 {
                for j in 0..NB {
                    let part = exp_s[s] * self.x_exp[s][j] / tot;
                    exp_bloc[j] += part;
                    if s < RET_GOODS {
                        exp_goods[j] += part;
                    }
                }
            }
        }
        self.exp_bloc = exp_bloc;
        self.exp_goods = exp_goods;
        match &self.base {
            Some(b) => {
                // The deflator of this month's imports and exports is the PT ratio of post_bloc (F4): the same one that prices
                // the imports in `clear` and in the import price index.
                let pt_ratio = self.pt_ratio;
                for i in 0..NB {
                    let q = self.imp_bloc[i] / (pt_ratio * self.r[i].exp());
                    self.q_prev[i] = if q.is_finite() && q > 0.0 { q } else { 0.0 };
                }
                let imp_tot: f64 = imp_s.iter().sum();
                let exp_tot: f64 = exp_s.iter().sum();
                self.imp_vol_idx = if b.imp0 > 0.0 { 100.0 * (imp_tot / pt_ratio) / b.imp0 } else { 100.0 };
                self.exp_vol_idx = if b.exp0 > 0.0 { 100.0 * (exp_tot / pt_ratio) / b.exp0 } else { 100.0 };
            }
            None => {
                self.q_prev = [0.0; NB];
            }
        }
        self.m_prev = imp_s;
        self.exp_prev = exp_s;
    }
}
