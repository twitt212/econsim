#!/usr/bin/env python3
"""Rates, yield curve and credit: model moments against US stylised facts.

Usage: python3 scripts/facts_rates.py [path to econsim-cli] [--dump-targets PATH]
  binary: first argument, else $ECONSIM_CLI, else ./target/release/econsim-cli (relative to the repository root)
  env:    ECONSIM_CFG (config override, passed to every run), RATES_HH (20000), RATES_SEEDS (8), RATES_MONTHS (240),
          RATES_JOBS (parallel runs, default = seeds); each run uses RAYON_NUM_THREADS=1.
  --dump-targets PATH writes the target table (values, ranges, samples, sources) as JSON and exits.

Model side: baseline runs after the 72-month burn-in, first 12 reported months dropped; quarterly moments use
3-month averages and HP cycles (lambda 1600, log real GDP). Per-seed statistics are averaged over seeds (the table
shows the mean and the across-seed sd); recession statistics and the policy-rule regression (seed intercepts) pool
all seeds. Recessions are dated from unemployment (Sahm-rule onsets; rises of >= 1pp within 18 months), identically in
model and US data, because the model's quarterly GDP is noisier than US GDP (q/q sd ~0.8% vs ~0.55%), so two-quarter
GDP declines in the model are mostly noise. US side: FRED series (H.15 CMT yields,
FEDFUNDS, Moody's Baa, Freddie Mac PMMS, Kim-Wright term premium, Fed charge-off rates, BEA GDPC1), the Fed's
Gilchrist-Zakrajsek spread file, FDIC QBP and the papers cited per row. Sample 1985-2019 unless stated (Great
Moderation, pre-COVID); the `alt` field records the 1962-2019 value where it differs materially.

Gap flag: ok inside the target range; otherwise r = distance outside the range / the row's tolerance `scale`:
minor r <= 0.5, material r <= 2, severe r > 2 (or the wrong sign on a sign fact).
"""
import json, os, subprocess, sys, time, warnings
from concurrent.futures import ThreadPoolExecutor
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
warnings.filterwarnings('ignore', category=RuntimeWarning)

# ----------------------------------------------------------------------------------------------- targets
# id, label, target value, [lo, hi], scale for the gap flag, sign fact?, sample, source, alt (other sample), note
TARGETS = [
    dict(id='policy_mean', label='Policy rate mean, %', value=3.0, lo=2.5, hi=3.5, scale=0.5, sample='2019-2025 FOMC SEP longer run',
         source='FOMC Summary of Economic Projections, longer-run fed funds median 2.5% (2019-23) to 3.0% (2025); '
                'Holston-Laubach-Williams r* ~0.75% + 2% target',
         alt='1985-2019 FEDFUNDS mean 3.59% (higher-inflation early years)'),
    dict(id='slope_mean', label='10y - 3m slope mean, pp', value=1.74, lo=1.4, hi=2.1, scale=0.4, sample='1985:01-2019:12',
         source='FRED GS10, GS3M (H.15 constant maturity); own calculation', alt='1962-2019 (TB3MS): 1.52'),
    dict(id='slope_30_10', label='30y - 10y slope mean, pp', value=0.48, lo=0.3, hi=0.7, scale=0.3, sample='1985:01-2019:12 (30y gap 2002-06)',
         source='FRED GS30, GS10; own calculation'),
    dict(id='slope_sd', label='sd of 10y - 3m slope, pp', value=1.12, lo=0.9, hi=1.4, scale=0.25, sample='1985:01-2019:12',
         source='FRED GS10, GS3M; own calculation', alt='1962-2019: 1.23'),
    dict(id='inverted_share', label='Share of months with 10y < 3m', value=0.062, lo=0.03, hi=0.12, scale=0.02, sample='1985:01-2019:12',
         source='FRED GS10, GS3M; own calculation', alt='1962-2019 (TB3MS): 0.10'),
    dict(id='sd_d3m', label='sd of monthly change, 3m yield, pp', value=0.20, lo=0.15, hi=0.25, scale=0.05, sample='1985:01-2019:12',
         source='FRED GS3M; own calculation (1990-2007: 0.205)', alt='1962-2019 (TB3MS): 0.43 (Volcker era)'),
    dict(id='sd_d10', label='sd of monthly change, 10y yield, pp', value=0.24, lo=0.20, hi=0.28, scale=0.05, sample='1985:01-2019:12',
         source='FRED GS10; own calculation (1990-2007: 0.223)', alt='1962-2019: 0.28'),
    dict(id='ratio_d10_d3m', label='sd(d10y) / sd(d3m)', value=1.21, lo=0.9, hi=1.4, scale=0.3, sample='1985:01-2019:12',
         source='FRED; excess volatility of long rates: Shiller (1979) JPE 87(6); Gurkaynak, Sack & Swanson (2005) AER 95(1)',
         alt='1962-2019: 0.66 (1979-82 short-rate volatility)'),
    dict(id='corr_d3m_d10', label='corr(d3m, d10y), monthly', value=0.45, lo=0.35, hi=0.60, scale=0.15, sample='1985:01-2019:12',
         source='FRED GS3M, GS10; own calculation (1990-2007: 0.41)', alt='1962-2019: 0.58'),
    dict(id='corr_di_d10', label='corr(d policy rate, d10y), monthly', value=0.26, lo=0.10, hi=0.40, scale=0.15, sample='1985:01-2019:12',
         source='FRED FEDFUNDS, GS10; own calculation (1990-2007: 0.16; corr with d2y 0.50): long yields move mostly on news '
         'other than the current policy move (Gurkaynak, Sack & Swanson 2005)'),
    dict(id='pca_level', label='PCA of monthly yield changes: level share', value=0.83, lo=0.78, hi=0.90, scale=0.05,
         sample='1985:01-2019:12, tenors 3m/2y/5y/10y/30y', source='FRED; own calculation. Litterman & Scheinkman (1991) '
         'J. Fixed Income 1(1): level ~89%, slope ~8%, curvature ~2%'),
    dict(id='pca_curv', label='PCA of monthly yield changes: curvature share', value=0.032, lo=0.015, hi=0.05, scale=0.005,
         sample='1985:01-2019:12', source='FRED; own calculation (slope share 0.13). Litterman & Scheinkman (1991)'),
    dict(id='cs_beta', label='Campbell-Shiller beta, 10y (m = 3m)', value=-1.35, lo=-3.0, hi=-0.5, scale=1.0, sign=True,
         sample='1985:01-2019:12', source='Campbell & Shiller (1991) REStud 58(3): beta < 0 and falling with maturity '
         '(expectations hypothesis implies +1); FRED GS10/GS3M own calculation with y(117m) ~ y(120m)', alt='1962-2019: -2.9'),
    dict(id='tp10_mean', label='10y term premium mean, pp', value=0.97, lo=0.5, hi=1.5, scale=0.4, sample='1990:01-2019:12',
         source='Kim & Wright (2005) FEDS 2005-33, FRED THREEFYTP10 (2000-2019 mean 0.60; ACM similar order)'),
    dict(id='tp10_dsd', label='sd of monthly change, 10y term premium, pp', value=0.11, lo=0.08, hi=0.15, scale=0.03,
         sample='1990:01-2019:12', source='FRED THREEFYTP10 (Kim-Wright); own calculation'),
    dict(id='tp_share_d10', label='cov(dTP10, d10y) / var(d10y)', value=0.48, lo=0.35, hi=0.65, scale=0.15, sample='1990:01-2019:12',
         source='FRED THREEFYTP10, GS10; own calculation (corr 0.94): about half of long-rate moves are term-premium moves'),
    dict(id='tp_cyc_corr', label='corr(TP10 cycle, GDP cycle)', value=-0.18, lo=-0.6, hi=-0.05, scale=0.25, sign=True,
         sample='1990Q1-2019Q4, HP cycles', source='FRED THREEFYTP10, GDPC1; own calculation; countercyclical term premia: '
         'Adrian, Crump & Moench (2013) JFE 110(1); Cochrane & Piazzesi (2005) AER 95(1)'),
    dict(id='rec_auc', label='AUC: -slope_t predicts u +1pp within 18m', value=0.81, lo=0.70, hi=0.90, scale=0.1,
         sample='1962:01-2019:12 (1985-2019: 0.73)', source='FRED UNRATE (3-month mean), GS10, TB3MS/GS3M; own calculation '
         '(NBER USREC 12m ahead: 0.87). Estrella & Mishkin (1998) REStat 80(1); Estrella & Hardouvelis (1991) JF 46(2)'),
    dict(id='rec_preceded', label='Share of Sahm-rule onsets after inversion (24m)', value=0.83, lo=0.7, hi=1.0, scale=0.2,
         sample='1962-2019 (1985-2019: 3/3)', source='FRED UNRATE, GS10, TB3MS; own calculation, leads 10-18 months. Bauer & '
         'Mertens (2018) FRBSF Economic Letter 2018-07: every recession in 60 years preceded by a 10y-3m inversion, one false positive'),
    dict(id='policy_cut', label='Policy change around Sahm onsets, pp', value=-4.0, lo=-5.5, hi=-2.5, scale=1.0, sign=True,
         sample='1962-2019 (1985-2019: -4.5)', source='FRED FEDFUNDS: rate 12 months after a Sahm-rule onset minus its 12-month '
         'pre-onset maximum (1990 -3.6, 2001 -4.8, 2008 -5.0); own calculation'),
    dict(id='taylor_rho', label='Policy rule smoothing rho (quarterly)', value=0.85, lo=0.75, hi=0.92, scale=0.1,
         sample='1987Q3-2007Q4', source='FRED FEDFUNDS, PCEPILFE, UNRATE: i_t = c + rho i_t-1 + b pi_t + d u_t (own estimate 0.85; model: pooled over seeds with seed intercepts, '
         'quarters above the zero bound); Clarida, Gali & Gertler (2000) QJE 115(1): rho ~0.79'),
    dict(id='taylor_phi_pi', label='Long-run response to core inflation', value=2.27, lo=1.5, hi=2.5, scale=0.5, sample='1987Q3-2007Q4',
         source='Own estimate as above (2.27); Clarida, Gali & Gertler (2000): ~2.15; Taylor (1993): 1.5'),
    dict(id='taylor_phi_u', label='Long-run response to unemployment', value=-2.2, lo=-2.5, hi=-1.0, scale=0.5, sample='1987Q3-2007Q4',
         source='Own estimate as above (-2.20); CGG output-gap 0.93 x Okun 2 ~ -1.9; Taylor (1993) 0.5 x 2 = -1.0'),
    dict(id='zlb_share', label='Share of months at the zero bound', value=0.13, lo=0.05, hi=0.40, scale=0.04, sample='1954-2023 / 1985-2023',
         source='FRED FEDFUNDS < 0.25: 13% of 1954-2023, 23% of 1985-2023; Kiley & Roberts (2017) BPEA: 30-40% when the '
         'neutral nominal rate is ~3%'),
    dict(id='spread_mean', label='Corporate credit spread mean, pp', value=2.0, lo=1.7, hi=2.5, scale=0.4, sample='1985:01-2019:12',
         source='Gilchrist & Zakrajsek (2012) AER 102(4) GZ spread, Fed ebp_csv (mean 2.01); Moody\'s Baa-10y FRED BAA10YM 2.32'),
    dict(id='spread_cyc_sd', label='Credit spread cycle sd (HP, quarterly), pp', value=0.55, lo=0.4, hi=0.7, scale=0.2,
         sample='1985Q1-2019Q4', source='GZ spread 0.64, Baa-10y 0.48; own calculation'),
    dict(id='spread_corr0', label='corr(spread cycle, GDP cycle), same quarter', value=-0.45, lo=-0.6, hi=-0.25, scale=0.2, sign=True,
         sample='1985Q1-2019Q4', source='Baa-10y -0.45, GZ -0.28 (1962-2019 Baa -0.58); own calculation; Gilchrist & Zakrajsek (2012)'),
    dict(id='spread_lead', label='corr(GDP_t, spread_t-2q): spreads lead', value=-0.57, lo=-0.70, hi=-0.40, scale=0.2, sign=True,
         sample='1985Q1-2019Q4', source='Baa-10y -0.61, GZ -0.53, EBP -0.46 (corr with GDP 2q later; lagging corr ~0 to +0.2); '
         'Gilchrist & Zakrajsek (2012): spreads and EBP predict activity'),
    dict(id='mort_spread_mean', label='30y mortgage - 10y spread mean, pp', value=1.69, lo=1.5, hi=1.9, scale=0.3, sample='1985:01-2019:12',
         source='FRED MORTGAGE30US (Freddie Mac PMMS) - GS10; own calculation (1990-2019: 1.66; 2022-23: 2.61)'),
    dict(id='mort_spread_sd', label='sd of mortgage - 10y spread, pp', value=0.31, lo=0.25, hi=0.45, scale=0.1, sample='1985:01-2019:12',
         source='FRED; own calculation: range 1.2-3.2, 2008 peak 2.9, 2022-23 peak 3.0; corr with Baa-10y 0.61. Fuster, Lo & '
         'Willen (2017) on time-varying intermediation spreads'),
    dict(id='co_mean', label='C&I loan charge-off rate mean, %/yr', value=0.82, lo=0.5, hi=1.1, scale=0.3, sample='1985Q1-2019Q4',
         source='Federal Reserve charge-off rates, FRED CORBLACBS (median 0.45, p90 1.7, max 2.6)'),
    dict(id='co_cyc', label='corr(charge-off cycle, GDP cycle)', value=-0.68, lo=-0.8, hi=-0.45, scale=0.2, sign=True,
         sample='1985Q1-2019Q4', source='FRED CORBLACBS, GDPC1, HP cycles; own calculation (peaks 0-1 quarters after GDP)'),
    dict(id='dep_beta_3m', label='Deposit beta, 3-month changes', value=0.15, lo=0.05, hi=0.25, scale=0.15, sample='2022 tightening',
         source='S&P Global Market Intelligence (2023): quarterly deposit-cost betas 18% (IB, 2022Q2), 24% (2022Q3), '
         '33% (2022Q4); cumulative ~0.22 after three quarters (+90bp costs vs ~+4pp fed funds)'),
    dict(id='dep_beta_24m', label='Deposit beta, 24-month changes', value=0.40, lo=0.30, hi=0.50, scale=0.1, sample='1994-2013 Call Reports',
         source='Drechsler, Savov & Schnabl (2017) QJE 132(4): mean deposit spread beta 0.54 with 4 quarterly lags, so deposit '
         'rate beta ~0.46 (savings deposits 0.24-0.34); 2022-23 cycle cumulative ~0.35-0.45'),
    dict(id='bank_cap', label='Bank risk-based capital ratio, %', value=15.0, lo=13.0, hi=16.0, scale=2.5, sample='2010-2024',
         source='FDIC Quarterly Banking Profile 2024Q1: total risk-based 15.4%, tier 1 14.0%, leverage 9.2%; equity/assets '
         '2010-19 11.2% (FRED EQTA)'),
    dict(id='bank_profit_gdp', label='Bank net income / GDP, %', value=1.0, lo=0.8, hi=1.2, scale=0.3, sample='2015-2023',
         source='FDIC QBP: net income $233.1bn (2019) and $256.9bn (2023) = 1.08% and 0.93% of GDP; ROA ~1.0-1.1% (FRED USROA)'),
    # Same target on the after-tax concept (FDIC net income is after tax). Needs bank_net_income_bn_ar (n/a on older binaries).
    dict(id='bank_ni_aftertax', label='Bank net income after tax / GDP, %', value=1.0, lo=0.8, hi=1.2, scale=0.3, sample='2015-2023',
         source='FDIC QBP net income (after taxes), as bank_profit_gdp; model: bank_net_income_bn_ar = bank profit less the '
         'bank corporate tax paid that month'),
]

# ----------------------------------------------------------------------------------------------- helpers
def hp(y, lam=1600.0):
    n = len(y); I = np.eye(n); D = np.diff(I, 2, axis=0)
    return y - np.linalg.solve(I + lam * D.T @ D, y)

def qavg(x):
    x = np.asarray(x, float); x = x[: len(x) // 3 * 3]
    return x.reshape(-1, 3).mean(1)

def corr(a, b):
    a, b = np.asarray(a, float), np.asarray(b, float)
    if len(a) < 3 or a.std() < 1e-12 or b.std() < 1e-12:
        return float('nan')
    return float(np.corrcoef(a, b)[0, 1])

def ols_slope(x, y):
    x, y = np.asarray(x, float), np.asarray(y, float)
    if len(x) < 3 or x.std() < 1e-12:
        return float('nan')
    return float(np.polyfit(x, y, 1)[0])

def lagcorr(y, x, k):
    """corr(y_t, x_{t+k}); k < 0: x leads y by |k|."""
    if k >= 0:
        return corr(y[: len(y) - k], x[k:])
    return corr(y[-k:], x[: len(x) + k])

def tech_recession(gq):
    """Quarters in technical recessions: two consecutive quarterly declines in real GDP (same rule as the US numbers)."""
    g = np.diff(np.log(gq)); r = np.zeros(len(gq), int)
    for t in range(1, len(g)):
        if g[t] < 0 and g[t - 1] < 0:
            r[t] = 1; r[t + 1] = 1
    return r

def ma3(x):
    x = np.asarray(x, float); return np.convolve(x, np.ones(3) / 3, mode='full')[: len(x)]

def sahm_onsets(u):
    """Months where the 3-month mean unemployment rate first reaches 0.5pp above its 12-month low (>= 24 months apart)."""
    s = ma3(u); on = []; active = False
    for t in range(14, len(s)):
        trig = s[t] - s[t - 12: t + 1].min() >= 0.5
        if trig and not active and (not on or t - on[-1] >= 24):
            on.append(t)
        active = trig
    return on

def surge_event(u, h=18, th=1.0):
    """1 if the 3-month mean unemployment rate rises at least `th` pp above its current value within `h` months."""
    s = ma3(u); return np.array([1 if s[t + 1: t + h + 1].max() - s[t] >= th else 0 for t in range(len(s) - h)])

def auc(score, lab):
    score, lab = np.asarray(score, float), np.asarray(lab, int)
    pos, neg = score[lab == 1], score[lab == 0]
    if len(pos) == 0 or len(neg) == 0:
        return float('nan')
    gt = (pos[:, None] > neg[None, :]).mean(); eq = (pos[:, None] == neg[None, :]).mean()
    return float(gt + 0.5 * eq)

# ----------------------------------------------------------------------------------------------- model
def run(cli, hh, months, seed):
    env = dict(os.environ, RAYON_NUM_THREADS='1')
    p = subprocess.run([cli, 'run', str(hh), str(months), str(seed), '--json'], capture_output=True, text=True, env=env)
    if p.returncode != 0:
        raise RuntimeError(f'seed {seed}: {p.stderr[-500:]}')
    return json.loads(p.stdout.strip().splitlines()[-1])

def series(h):
    a = lambda k: np.array([m[k] for m in h], float)
    Y = np.array([m['yields'] for m in h], float)
    TP = np.array([m['term_premium'] for m in h], float)
    return dict(i=a('policy_rate'), Y=Y, TP=TP, mort=a('mortgage_rate'), dep=a('deposit_rate'), spread=a('credit_spread'),
                co=(a('biz_chargeoff_3m_ar') if 'biz_chargeoff_3m_ar' in h[0] else a('loan_loss_rate')), cap=a('bank_capital_ratio'), bprof=a('bank_profits_bn_ar'), ngdp=a('gdp_nominal_bn_ar'),
                bni=(a('bank_net_income_bn_ar') if 'bank_net_income_bn_ar' in h[0] else np.full(len(h), np.nan)),
                rgdp=a('gdp_real_bn_ar'), u=a('unemployment_rate'), pi=a('core_pce_inflation_yoy'))

def seed_stats(s):
    Y, i = s['Y'], s['i']; y3, y10, y30 = Y[:, 0], Y[:, 3], Y[:, 4]; sl = y10 - y3
    dY = np.diff(Y, axis=0); d3, d10 = dY[:, 0], dY[:, 3]
    ev = np.sort(np.linalg.eigvalsh(np.cov(dY.T)))[::-1]; ev = ev / ev.sum() if ev.sum() > 0 else ev * np.nan
    tp10 = s['TP'][:, 3]; dtp = np.diff(tp10)
    yq = np.log(qavg(s['rgdp'])); yc = hp(yq)
    o = {}
    o['policy_mean'] = i.mean()
    o['slope_mean'] = sl.mean(); o['slope_30_10'] = (y30 - y10).mean(); o['slope_sd'] = sl.std(); o['inverted_share'] = (sl < 0).mean()
    o['sd_d3m'] = d3.std(); o['sd_d10'] = d10.std(); o['ratio_d10_d3m'] = d10.std() / d3.std() if d3.std() > 0 else float('nan')
    o['corr_d3m_d10'] = corr(d3, d10); o['corr_di_d10'] = corr(np.diff(i), d10); o['pca_level'] = ev[0]; o['pca_curv'] = ev[2]
    o['cs_beta'] = ols_slope((3 / 117) * (y10[:-3] - y3[:-3]), y10[3:] - y10[:-3])
    o['tp10_mean'] = tp10.mean(); o['tp10_dsd'] = dtp.std()
    o['tp_share_d10'] = float(np.cov(dtp, d10)[0, 1] / d10.var()) if d10.var() > 0 else float('nan')
    o['tp_cyc_corr'] = corr(hp(qavg(tp10)), yc)
    # policy rule, quarterly, away from the zero bound (pooled across seeds in main)
    iq, piq, uq = qavg(i), qavg(s['pi']), qavg(s['u'])
    keep = (iq[1:] > 0.25) & (iq[:-1] > 0.25)
    o['_taylor'] = (np.column_stack([iq[:-1][keep], piq[1:][keep], uq[1:][keep]]), iq[1:][keep])
    o['zlb_share'] = (i < 0.125).mean()
    sp = s['spread']; spc = hp(qavg(sp))
    o['spread_mean'] = sp.mean(); o['spread_cyc_sd'] = spc.std(); o['spread_corr0'] = corr(spc, yc); o['spread_lead'] = lagcorr(yc, spc, -2)
    ms = s['mort'] - y10; o['mort_spread_mean'] = ms.mean(); o['mort_spread_sd'] = ms.std()
    o['co_mean'] = s['co'].mean(); o['co_cyc'] = corr(hp(qavg(s['co'])), yc)
    dep = s['dep']
    o['dep_beta_3m'] = ols_slope(i[3:] - i[:-3], dep[3:] - dep[:-3]); o['dep_beta_24m'] = ols_slope(i[24:] - i[:-24], dep[24:] - dep[:-24])
    o['bank_cap'] = s['cap'].mean(); o['bank_profit_gdp'] = 100 * np.mean(s['bprof'] / s['ngdp'])
    o['bank_ni_aftertax'] = 100 * np.mean(s['bni'] / s['ngdp'])
    # pooled recession inputs: unemployment surges (robust to the noise in quarterly GDP), Sahm-rule onsets
    evt = surge_event(s['u']); o['_auc'] = (-sl[: len(evt)], evt)
    on = [t for t in sahm_onsets(s['u']) if t >= 24]
    o['_rec'] = [bool((sl[t - 24: t] < 0).any()) for t in on]
    o['_cut'] = [i[t + 12] - i[t - 12: t + 1].max() for t in on if t + 12 < len(i)]
    rq = tech_recession(qavg(s['rgdp'])); starts = [t for t in range(1, len(rq)) if rq[t] == 1 and rq[t - 1] == 0]
    o['_extra'] = dict(n_rec=len(starts), n_sahm=len(on), gq_sd=float(np.diff(np.log(qavg(s['rgdp']))).std() * 100), slope_cyc_corr=corr(hp(qavg(sl)), yc), spread_lag=lagcorr(yc, spc, 2),
                       mort_corr_spread=corr(ms, sp), ac1_i=corr(i[1:], i[:-1]), months_rate_change=float((np.abs(np.diff(i)) > 1e-9).mean()),
                       mean_y=Y.mean(0).tolist(), sd_level_y=Y.std(0).tolist(), pca_slope=ev[1], d_i_ac1=corr(np.diff(i)[1:], np.diff(i)[:-1]),
                       corr_d_i_d10=corr(np.diff(i), d10), tp_mean_all=s['TP'].mean(0).tolist(), dep_mean=dep.mean())
    return o

def flag(t, m):
    if m is None or not np.isfinite(m):
        return 'unmeasurable'
    lo, hi = t['lo'], t['hi']
    if t.get('sign') and np.sign(m) != np.sign(t['value']) and abs(m) > 0.05:
        return 'severe'
    if lo <= m <= hi:
        return 'ok'
    d = (lo - m) if m < lo else (m - hi)
    r = d / t['scale']
    return 'minor' if r <= 0.5 else 'material' if r <= 2 else 'severe'

def main():
    args = [a for a in sys.argv[1:]]
    if '--dump-targets' in args:
        k = args.index('--dump-targets'); path = args[k + 1]
        with open(path, 'w') as f:
            json.dump(dict(dimension='rates, yield curve and credit', script='scripts/facts_rates.py', targets=TARGETS), f, indent=1)
        print('wrote', path); return
    cli = args[0] if args else os.environ.get('ECONSIM_CLI', os.path.join(HERE, '..', 'target', 'release', 'econsim-cli'))
    hh = int(os.environ.get('RATES_HH', '20000')); seeds = int(os.environ.get('RATES_SEEDS', '8'))
    months = int(os.environ.get('RATES_MONTHS', '240')); jobs = int(os.environ.get('RATES_JOBS', str(seeds)))
    t0 = time.time()
    with ThreadPoolExecutor(jobs) as ex:
        runs = list(ex.map(lambda sd: run(cli, hh, months, sd), range(1, seeds + 1)))
    wall = time.time() - t0
    stats = [seed_stats(series(h[12:])) for h in runs]
    print(f"facts_rates: {seeds} seeds x {months} months x {hh} households (first 12 months dropped); wall {wall:.1f}s; "
          f"ECONSIM_CFG={os.environ.get('ECONSIM_CFG', '')!r}; cli={cli}")
    sc = np.concatenate([o['_auc'][0] for o in stats]); lb = np.concatenate([o['_auc'][1] for o in stats])
    recs = sum((o['_rec'] for o in stats), [])
    cuts = sum((o['_cut'] for o in stats), [])
    pooled = {'rec_auc': auc(sc, lb), 'rec_preceded': (float(np.mean(recs)) if recs else float('nan')),
              'policy_cut': (float(np.mean(cuts)) if cuts else float('nan'))}
    # pooled policy rule with seed intercepts
    Xs, ys = [], []
    for k, o in enumerate(stats):
        X, y = o['_taylor']; d = np.zeros((len(y), len(stats))); d[:, k] = 1; Xs.append(np.column_stack([d, X])); ys.append(y)
    X, y = np.vstack(Xs), np.concatenate(ys)
    if len(y) > len(stats) + 8:
        c, *_ = np.linalg.lstsq(X, y, rcond=None); rho = c[-3]
        pooled.update(taylor_rho=rho, taylor_phi_pi=c[-2] / (1 - rho), taylor_phi_u=c[-1] / (1 - rho))
    rows = []
    print(f"{'fact':48s} {'target [range]':>22s} {'model (sd seeds)':>20s}  gap")
    for t in TARGETS:
        if t['id'] in pooled:
            m, sd = pooled[t['id']], float('nan')
        else:
            v = np.array([o.get(t['id'], float('nan')) for o in stats], float)
            m = float(np.nanmean(v)) if np.isfinite(v).any() else float('nan'); sd = float(np.nanstd(v)) if np.isfinite(v).any() else float('nan')
        g = flag(t, m); rows.append(dict(id=t['id'], label=t['label'], target=t['value'], lo=t['lo'], hi=t['hi'], model=m, model_sd=sd, gap=g))
        tgt = f"{t['value']:.3g} [{t['lo']:.3g}, {t['hi']:.3g}]"
        mod = f"{m:.3g}" + (f" ({sd:.2g})" if np.isfinite(sd) else '')
        print(f"{t['label']:48s} {tgt:>22s} {mod:>20s}  {g}")
    ex = [o['_extra'] for o in stats]
    mean = lambda k: float(np.nanmean([e[k] for e in ex]))
    print(f"\n{len(recs)} Sahm-rule onsets in {seeds * (months - 12) / 12:.0f} model years (US 1962-2019: 6 in 58); "
          f"{sum(e['n_rec'] for e in ex)} technical recessions (US: 6 in 58 years); q/q real GDP growth sd {mean('gq_sd'):.2f}% "
          f"(US 1985-2019 ~0.55%); AUC over {len(lb)} months")
    print('mean yields 3m/2y/5y/10y/30y:', ' '.join(f"{v:.2f}" for v in np.mean([e['mean_y'] for e in ex], 0)),
          ' | level sd:', ' '.join(f"{v:.2f}" for v in np.mean([e['sd_level_y'] for e in ex], 0)),
          ' | term premia:', ' '.join(f"{v:.2f}" for v in np.mean([e['tp_mean_all'] for e in ex], 0)))
    print(f"PCA slope share {mean('pca_slope'):.3f}; corr(slope cycle, GDP cycle) {mean('slope_cyc_corr'):+.2f}; corr(GDP_t, spread_t+2q) "
          f"{mean('spread_lag'):+.2f} (US Baa +0.03, GZ +0.20); corr(mortgage spread, credit spread) {mean('mort_corr_spread'):+.2f} (US 0.61)")
    print(f"policy: share of months with a change {mean('months_rate_change'):.2f}; ac1 of monthly changes {mean('d_i_ac1'):+.2f} "
          f"(US 1987-2007 0.57); corr(d policy, d 10y) {mean('corr_d_i_d10'):+.2f}; mean deposit rate {mean('dep_mean'):.2f}")
    out = os.environ.get('RATES_OUT')
    if out:
        with open(out, 'w') as f:
            json.dump(dict(wall_s=wall, seeds=seeds, months=months, households=hh, rows=rows, extra=ex), f, indent=1, default=float)

if __name__ == '__main__':
    main()
