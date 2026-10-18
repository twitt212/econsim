//! The isolated oil market checks (`econsim-cli oil-check`, spec 3 Stage 3 measurement (a)): the research's K1 to K8 tables for
//! the aggregate market (one region, the prototype's activity process drawn on `WORLD_OIL_S`), reproduced with the Rust market.
//! The bands and centres are the research check table (golden `bands`), the 2024 balance is the golden `balance_2024`, and the
//! K6 replay and the K7 scenarios are the golden vectors of `oil_reference.py`. Samples are keyed by their index and run in
//! parallel (rayon), so the tables are independent of the thread count.
//!
//! One difference from the research script: the jump counts are exact Poisson draws (`exog::poisson`), where the research
//! thresholds a uniform to at most two jumps; the difference is far inside the bands (checked in the RESULT).

use rayon::prelude::*;
use serde_json::Value;

use super::oil::{self, OilInputs, OilParams, OilShocks, OilState};
use crate::config::Config;
use crate::rng::{self, normal, stream};

const GOLD_OIL: &str = include_str!("data/golden/oil_golden.json");

/// Which shocks are active in a run (the innovations are always drawn, so the runs share their draws: common random numbers).
#[derive(Clone, Copy, Debug)]
pub struct Mask {
    pub i: bool,
    pub d: bool,
    pub o: bool,
    pub y: bool,
}

impl Mask {
    pub const ALL: Mask = Mask { i: true, d: true, o: true, y: true };
    pub fn only(s: &str) -> Mask {
        Mask { i: s == "I", d: s == "D", o: s == "O", y: s == "y" }
    }
}

/// The isolated aggregate parameters of `oil-check` from the config (profile, levers, activity process of `oil-check`).
pub fn aggregate_params(cfg: &Config) -> OilParams {
    let t = super::params::WorldParams::load().oil.clone();
    OilParams::new(&t, cfg, true)
}

/// One simulated path of the isolated market: `months` months after `burn`, with the activity AR(1) of `oil-check` (sigma and rho
/// from the config). Returns the monthly-average price (% log deviation) and the supply deviation, after the burn-in.
pub fn sample(p: &OilParams, cfg: &Config, index: u64, months: usize, burn: usize, mask: Mask) -> (Vec<f64>, Vec<f64>) {
    let mut ri = rng::agent_rng(index + 1, 0, stream::WORLD_OIL_I, 0);
    let mut rs = rng::agent_rng(index + 1, 0, stream::WORLD_OIL_S, 0);
    let mut gar = oil::DrawState::default();
    let mut s = OilState::new(p);
    let mut y = 0.0;
    let sig_y = cfg.world_oilcheck_activity_sigma * cfg.world_oil_sigma_mult;
    let rho_y = cfg.world_oilcheck_activity_rho;
    let mut pbar = Vec::with_capacity(months - burn);
    let mut supply = Vec::with_capacity(months - burn);
    for t in 0..months {
        let e_i = oil::draw_innovation(p, &mut gar, &mut ri);
        let (e_d, e_o) = oil::draw_supply(p, &mut rs);
        let z = normal(&mut rs);
        let e_y = sig_y * z;
        y = rho_y * y + if mask.y { e_y } else { 0.0 };
        let sh = OilShocks {
            e_i: if mask.i { e_i } else { 0.0 },
            e_d: if mask.d { e_d } else { 0.0 },
            e_o: if mask.o { e_o } else { 0.0 },
            ..Default::default()
        };
        let inp = OilInputs { y: vec![y], rfx: vec![0.0], ..Default::default() };
        let out = s.step(p, &inp, &sh);
        if t >= burn {
            pbar.push(out.pbar);
            supply.push(out.supply);
        }
    }
    (pbar, supply)
}

/// The research moments of one path (the research simulator's `moments_vec`, computed per path; the median is taken over paths).
#[derive(Clone, Copy, Debug, Default)]
pub struct Moments {
    pub sd: f64,
    pub skew: f64,
    pub exkurt: f64,
    pub ac1: f64,
    pub vr12: f64,
    pub ar1_level: f64,
    pub sd_level: f64,
    pub sd_level_20y: f64,
    pub share_big12: f64,
    pub share_gt3sd: f64,
}

fn mean(x: &[f64]) -> f64 {
    x.iter().sum::<f64>() / x.len() as f64
}

fn var0(x: &[f64]) -> f64 {
    let m = mean(x);
    x.iter().map(|v| (v - m) * (v - m)).sum::<f64>() / x.len() as f64
}

pub fn moments(pbar: &[f64]) -> Moments {
    let d: Vec<f64> = pbar.windows(2).map(|w| w[1] - w[0]).collect();
    let m = mean(&d);
    let s = var0(&d).sqrt();
    let zc: Vec<f64> = d.iter().map(|v| (v - m) / s).collect();
    let sk = mean(&zc.iter().map(|z| z * z * z).collect::<Vec<_>>());
    let ku = mean(&zc.iter().map(|z| z * z * z * z).collect::<Vec<_>>()) - 3.0;
    let ac1 = d.windows(2).map(|w| (w[1] - m) * (w[0] - m)).sum::<f64>() / (d.len() - 1) as f64 / (s * s);
    let d12: Vec<f64> = (12..pbar.len()).map(|t| pbar[t] - pbar[t - 12]).collect();
    let vr12 = var0(&d12) / (12.0 * var0(&d));
    let pm = mean(pbar);
    let pc: Vec<f64> = pbar.iter().map(|v| v - pm).collect();
    let ar1 = pc.windows(2).map(|w| w[0] * w[1]).sum::<f64>() / (pc.len() - 1) as f64 / var0(&pc);
    let w = 240usize;
    let mut sdw = Vec::new();
    let mut a = 0usize;
    while a + w <= pbar.len() {
        sdw.push(var0(&pbar[a..a + w]).sqrt() / 100.0);
        a += 24;
    }
    let share_big = d12.iter().filter(|x| x.abs() > 50.0).count() as f64 / d12.len() as f64;
    let g3 = zc.iter().filter(|z| z.abs() > 3.0).count() as f64 / zc.len() as f64;
    Moments {
        sd: s,
        skew: sk,
        exkurt: ku,
        ac1,
        vr12,
        ar1_level: ar1,
        sd_level: var0(pbar).sqrt() / 100.0,
        sd_level_20y: if sdw.is_empty() { var0(pbar).sqrt() / 100.0 } else { mean(&sdw) },
        share_big12: share_big,
        share_gt3sd: g3,
    }
}

pub fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// Median of the moments over `n` keyed samples of `months` months after `burn` (K4).
pub fn median_moments(p: &OilParams, cfg: &Config, n: usize, months: usize, burn: usize) -> Moments {
    let ms: Vec<Moments> = (0..n as u64).into_par_iter().map(|i| moments(&sample(p, cfg, i, months, burn, Mask::ALL).0)).collect();
    let med = |f: fn(&Moments) -> f64| median(ms.iter().map(f).collect());
    Moments {
        sd: med(|m| m.sd),
        skew: med(|m| m.skew),
        exkurt: med(|m| m.exkurt),
        ac1: med(|m| m.ac1),
        vr12: med(|m| m.vr12),
        ar1_level: med(|m| m.ar1_level),
        sd_level: med(|m| m.sd_level),
        sd_level_20y: med(|m| m.sd_level_20y),
        share_big12: med(|m| m.share_big12),
        share_gt3sd: med(|m| m.share_gt3sd),
    }
}

/// Mean over samples of the variance of `h`-month price changes, for one active shock (K5, the variance shares).
pub fn variance_of_changes(p: &OilParams, cfg: &Config, n: usize, months: usize, burn: usize, h: usize, mask: Mask) -> f64 {
    let v: Vec<f64> = (0..n as u64)
        .into_par_iter()
        .map(|i| {
            let (x, _) = sample(p, cfg, i, months, burn, mask);
            let d: Vec<f64> = (h..x.len()).map(|t| x[t] - x[t - h]).collect();
            var0(&d)
        })
        .collect();
    mean(&v)
}

/// The unit impulse of the prototype (the innovation at month 0 and nothing after), `h` months. `shock` is I, D, O, y or news.
pub fn impulse(p: &OilParams, shock: &str, size: f64, h: usize) -> Vec<oil::OilOut> {
    let mut s = OilState::new(p);
    let mut y_path = 0.0_f64;
    let mut out = Vec::with_capacity(h);
    for t in 0..h {
        let mut sh = OilShocks::default();
        let mut y = 0.0;
        match shock {
            "I" if t == 0 => sh.e_i = size,
            "D" if t == 0 => sh.e_d = size,
            "O" if t == 0 => sh.e_o = size,
            "news" if t == 0 => sh.nu = size,
            "y" => {
                y_path = if t == 0 { size } else { p.rho_y * y_path };
                y = y_path;
            }
            _ => {}
        }
        out.push(s.step(p, &OilInputs { y: vec![y; p.n], rfx: vec![0.0; p.n], ..Default::default() }, &sh));
    }
    out
}

/// One K2/K3 check row: the model value, the research centre and band, and whether it is inside.
#[derive(Clone, Debug)]
pub struct BandRow {
    pub check: String,
    pub h: usize,
    pub model: f64,
    pub center: f64,
    pub lo: f64,
    pub hi: f64,
    pub inside: bool,
}

pub fn golden() -> Value {
    serde_json::from_str(GOLD_OIL).expect("oil golden parses")
}

fn band_rows(name: &str, values: &dyn Fn(usize) -> f64, g: &Value) -> Vec<BandRow> {
    g["bands"][name]
        .as_array()
        .expect("band rows")
        .iter()
        .map(|r| {
            let h = r["h"].as_u64().unwrap() as usize;
            let (lo, hi) = (r["lo"].as_f64().unwrap(), r["hi"].as_f64().unwrap());
            let model = values(h);
            BandRow {
                check: name.to_string(),
                h,
                model,
                center: r["center"].as_f64().unwrap(),
                lo,
                hi,
                inside: lo - 1e-9 <= model && model <= hi + 1e-9,
            }
        })
        .collect()
}

/// The K2 and K3 checks (the research check script, not published): IRFs scaled to the research centres, then the bands.
pub fn irf_checks(p: &OilParams, g: &Value) -> Vec<BandRow> {
    let h = 61;
    let unit_i = impulse(p, "I", 1.0, h);
    let h0_unit = unit_i[0].pbar;
    let s_i = g["bands"]["oil_specific_price"][0]["center"].as_f64().unwrap() / h0_unit;
    let r_i = impulse(p, "I", s_i, h);
    let unit_y = impulse(p, "y", 1.0, h);
    let c_ad: Vec<f64> = [3usize, 6, 12]
        .iter()
        .map(|&hh| g["bands"]["agg_demand_price"].as_array().unwrap().iter().find(|r| r["h"].as_u64() == Some(hh as u64)).unwrap()["center"].as_f64().unwrap())
        .collect();
    let s_y = c_ad.iter().sum::<f64>() / 3.0 / ([3usize, 6, 12].iter().map(|&hh| unit_y[hh].pbar).sum::<f64>() / 3.0);
    let r_y = impulse(p, "y", s_y, h);
    let r_d = impulse(p, "D", 1.25, h);
    let unit_n = impulse(p, "news", 1.0, h);
    let s_n = 10.0 / unit_n[0].pbar;
    let r_n = impulse(p, "news", s_n, h);
    let mut rows = Vec::new();
    rows.extend(band_rows("oil_specific_price", &|hh| r_i[hh].pbar, g));
    rows.extend(band_rows("agg_demand_price", &|hh| r_y[hh].pbar, g));
    rows.extend(band_rows("supply_production", &|hh| r_d[hh].supply, g));
    rows.extend(band_rows("supply_price(Cholesky bands)", &|hh| r_d[hh].pbar, g));
    rows.extend(band_rows("oil_specific_production", &|hh| r_i[hh].supply, g));
    rows.extend(band_rows("kanzig_price", &|hh| r_n[hh].pbar, g));
    rows.extend(band_rows("kanzig_production", &|hh| r_n[hh].supply, g));
    rows
}

/// The scaled IRF sizes used by the K2 checks (for the report): sI, sy, sn.
pub fn irf_scales(p: &OilParams, g: &Value) -> (f64, f64, f64) {
    let unit_i = impulse(p, "I", 1.0, 1);
    let s_i = g["bands"]["oil_specific_price"][0]["center"].as_f64().unwrap() / unit_i[0].pbar;
    let unit_y = impulse(p, "y", 1.0, 61);
    let c_ad: f64 = [3usize, 6, 12]
        .iter()
        .map(|&hh| g["bands"]["agg_demand_price"].as_array().unwrap().iter().find(|r| r["h"].as_u64() == Some(hh as u64)).unwrap()["center"].as_f64().unwrap())
        .sum::<f64>()
        / 3.0;
    let s_y = c_ad / ([3usize, 6, 12].iter().map(|&hh| unit_y[hh].pbar).sum::<f64>() / 3.0);
    let unit_n = impulse(p, "news", 1.0, 1);
    (s_i, s_y, 10.0 / unit_n[0].pbar)
}

/// K6: the 2026 Gulf replay with the STEO production path and no precautionary shock (golden rows): RMSE over March-August.
pub fn replay_k6(p: &OilParams, g: &Value) -> (Vec<f64>, f64) {
    let rows = g["k6"]["rows"].as_array().unwrap();
    let mut s = OilState::new(p);
    let mut errs = Vec::new();
    let b0 = g["k6"]["baseline_brent"].as_f64().unwrap();
    for r in rows {
        let sup = r["supply_override"].as_f64().unwrap();
        let out = s.step(p, &OilInputs { y: vec![0.0; p.n], rfx: vec![0.0; p.n], supply_override: Some(sup), ..Default::default() }, &OilShocks::default());
        errs.push(out.pbar - 100.0 * (r["brent_obs"].as_f64().unwrap() / b0).ln());
    }
    let rmse = (errs[..6].iter().map(|e| e * e).sum::<f64>() / 6.0).sqrt();
    (errs, rmse)
}

/// K1 identity checks: the clearing residual over a stochastic path, the zero-shock drift, and the 2024 balance.
pub fn identity_k1(p: &OilParams, g: &Value) -> (f64, f64, f64) {
    let mut s = OilState::new(p);
    let mut gar = oil::DrawState::default();
    let mut r = rng::agent_rng(0, 0, stream::WORLD_OIL_I, 0);
    let mut worst: f64 = 0.0;
    for _ in 0..300 {
        let e_i = oil::draw_innovation(p, &mut gar, &mut r);
        let out = s.step(p, &OilInputs { y: vec![0.0; p.n], rfx: vec![0.0; p.n], ..Default::default() }, &OilShocks { e_i, ..Default::default() });
        worst = worst.max((out.supply - out.demand - out.di).abs());
    }
    let mut z = OilState::new(p);
    let mut drift: f64 = 0.0;
    for _ in 0..120 {
        let out = z.step(p, &OilInputs { y: vec![0.0; p.n], rfx: vec![0.0; p.n], ..Default::default() }, &OilShocks::default());
        drift = drift.max(out.pbar.abs());
    }
    (worst, drift, g["balance_2024"]["gap_mbd"].as_f64().unwrap())
}

/// The K8 elasticities (the research check script, K8): demand within the month, at 12 months and long run, per 1 % price.
pub fn elasticities(p: &OilParams) -> (f64, f64, f64) {
    let within = p.eta0;
    let mut z = 0.0;
    let mut at12 = 0.0;
    for t in 0..240 {
        z = p.rho_z * z + (1.0 - p.rho_z) * p.eta1;
        if t == 11 {
            at12 = p.eta0 + z;
        }
    }
    (within, at12, p.eta0 + p.eta1)
}
