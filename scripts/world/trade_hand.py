#!/usr/bin/env python3
"""Hand values for the stage-2 trade block (spec section 2.5): ONE set of numbers, computed from the research params with the spec's equations.
The Rust unit tests `dcp_pass_through_hand_values`, `exports_price_and_fx_hand_values` and `import_volume_hand_values` read the golden `trade_hand.json`
(paths.golden('trade_hand')), written by `python3 -B trade_hand.py --write` at FULL precision, with monthly paths h = 0..60 (the spec quotes the printed table).
base_s of the export price term (spec 2.5) is zero here: the US sector price relative to PT is at its base.

Conventions (spec 2.5):
  * month index h: the shock acts from month 0; the value "at month h" is the state after h+1 monthly updates (impact month included).
  * Dollar +10%  =>  z~_j = 100 ln(1.10) = 9.531 for every bloc j (the Dollar shock is a log level); no income effect, no sector price response.
  * import price: r_j = -[theta_j Phi^d_j + (1-theta_j) Phi^nd_j]/100, Phi^d += 0.08 (0.47 z~ - Phi^d), Phi^nd += 0.90 (0.95 z~ - Phi^nd).
  * theta_bar = import-weighted USD-invoicing share of NON-ENERGY imports (energy has r = 0): sum_j omega0_j theta_j.
  * exports: X change = exp(lnA^fx + lnA^rp) - 1 with the destination weights of the 2024 export matrix; competitor index over exporters k != US, j with
    m_jk = import_shares_2024[k][j] (stored [exporter][importer], columns sum to 1).
  * real imports: import-share factor of goods.rs, ratio^sr * exp(z)^(lr-sr), z the lagged log terms ratio (speed 0.08) - the ABM's own mechanism,
    before demand effects; 'ratio' = 1/imp_rel = exp(PT z~/100).
"""
import os, sys, json, math, argparse
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__)); sys.path.insert(0, HERE)
import paths
TRADE = paths.research_json('trade')
BLOCS = ['EA', 'CN', 'JP', 'UK', 'CA', 'MX', 'IN', 'KR', 'BR', 'AU', 'OPEC+', 'RoW']
SECT = ['Energy', 'Food', 'Durables', 'Nondurables', 'Tech', 'Leisure', 'HealthEdu', 'OtherServices', 'Construction']
GDP_2024 = 29381.0
KD, SD, KN, SN = 0.47, 0.08, 0.95, 0.90                 # world_dcp_kappa_usd, world_dcp_speed_usd, world_dcp_kappa_nonusd, world_dcp_speed_nonusd
FX_SR, FX_LR, FX_SPEED = 0.15, 0.30, 0.04               # world_export_fx_sr / _lr / _speed
PR_SR, PR_LR = 0.10, 0.90                               # world_export_price_sr / _lr  (speed = FX_SPEED = world_export_speed)
Z10 = 100.0 * math.log(1.10)

_P = json.load(open(TRADE))
THETA = np.array([_P['invoicing']['theta_usd_share_of_bloc_exports_to_US'][b]['value'] for b in BLOCS])
_IMP = _P['sector_shares']['matrices']['imports_base_C26_in_Tech']['usd_bn_2024']
_EXP = _P['sector_shares']['matrices']['exports_base_C26_in_Tech']['usd_bn_2024']
M0 = np.array([[_IMP[b][s] for s in SECT] for b in BLOCS])        # origin x sector, USD bn
X0 = np.array([[_EXP[b][s] for s in SECT] for b in BLOCS])        # destination x sector
OMEGA = M0[:, 1:].sum(1) / M0[:, 1:].sum()                          # origin shares of non-energy imports
THETA_BAR = float((OMEGA * THETA).sum())
_G = _P['bloc_goods_matrix']['import_shares_2024']
MJK = np.array([[_G[k][j] for k in BLOCS] for j in BLOCS])         # [importer j][exporter k] = import_shares_2024[k][j]
W_DEST = X0.sum(1) / X0.sum()                                       # destination weights of US exports


def pass_through(h, theta_bar=None, per_origin=False):
    """Cumulative response of the non-energy import price index to a permanent +1 (log pt) z~ of every bloc, at month h (h+1 updates)."""
    n = h + 1
    phid = KD * (1 - (1 - SD) ** n); phind = KN * (1 - (1 - SN) ** n)
    if per_origin: return float((OMEGA * (THETA * phid + (1 - THETA) * phind)).sum())
    tb = THETA_BAR if theta_bar is None else theta_bar
    return tb * phid + (1 - tb) * phind


def import_price_index(h, z=Z10):
    """% change of the non-fuel import price index at month h for Dollar +10%."""
    return 100.0 * math.expm1(-pass_through(h) * z / 100.0)


def export_path(T=37, z=Z10, price_sr=PR_SR, price_lr=PR_LR, fx_sr=FX_SR, fx_lr=FX_LR, speed=FX_SPEED):
    """% change of US exports (price + exchange-rate channels only) at months 0..T-1 for Dollar +10%."""
    phid = np.zeros(12); phind = np.zeros(12); uf = np.zeros(12); up = np.zeros(12); out = []
    for t in range(T):
        phid += SD * (KD * z - phid); phind += SN * (KN * z - phind)
        r = -(THETA * phid + (1 - THETA) * phind) / 100.0
        uf += speed * (z / 100.0 - uf)
        lnfx = -(fx_sr * z / 100.0 + (fx_lr - fx_sr) * uf)
        rc = np.zeros(12)
        for j in range(12):
            msk = np.ones(12, bool); msk[j] = False
            rc[j] = (MJK[j, msk] * r[msk]).sum() / MJK[j, msk].sum()
        up += speed * (rc - up)
        lnrp = price_sr * rc + (price_lr - price_sr) * up
        out.append(100.0 * float((W_DEST * np.expm1(lnfx + lnrp)).sum()))
    return out


def import_volume_path(sr, lr, T=61, z=Z10, speed=0.08):
    """% change of the import share (real imports before demand effects) at months 0..T-1 for Dollar +10% with the ABM's own elasticity mechanism."""
    zl = 0.0; out = []
    for t in range(T):
        ln_ratio = pass_through(t) * z / 100.0                         # ln(terms ratio): import price falls by PT*z
        zl = ln_ratio if t == 0 else zl + speed * (ln_ratio - zl)      # goods.rs lagged_log starts at the current value
        out.append(100.0 * math.expm1(sr * ln_ratio + (lr - sr) * zl))
    return out


def table():
    """Summary (printed, rounded) and paths (golden, full precision)."""
    t = {}
    t['theta_bar_nonenergy'] = THETA_BAR
    t['theta_bar_all_sectors'] = float(((M0.sum(1) / M0.sum()) * THETA).sum())
    t['export_total_pct_gdp'] = float(X0.sum() / GDP_2024 * 100)
    t['z10'] = Z10
    t['sector_export_share_pct_gdp'] = {s: float(v) for s, v in zip(SECT, X0.sum(axis=0) / GDP_2024 * 100)}
    H = list(range(61))
    t['months'] = H
    t['pass_through_path'] = [pass_through(h) for h in H]
    t['pass_through_per_origin_path'] = [pass_through(h, per_origin=True) for h in H]
    t['import_price_index_pct_dollar10_path'] = [import_price_index(h) for h in H]
    t['exports_pct_dollar10_path'] = [float(x) for x in export_path(T=61)]
    t['import_volume_pct_dollar10_path'] = {f'{sr}/{lr}': [float(x) for x in import_volume_path(sr, lr, T=61)] for sr, lr in ((0.2, 0.6), (0.4, 1.0), (0.7, 1.3))}
    return t


def summary(t):
    p = lambda a, hs: {str(h): round(a[h], 4 if a is t['pass_through_path'] else 3) for h in hs}
    out = {'theta_bar_nonenergy': round(t['theta_bar_nonenergy'], 4), 'theta_bar_all_sectors': round(t['theta_bar_all_sectors'], 4), 'z10': round(t['z10'], 3),
           'export_total_pct_gdp': round(t['export_total_pct_gdp'], 3)}
    out['pass_through'] = p(t['pass_through_path'], (0, 1, 3, 6, 12, 24, 36))
    out['import_price_index_pct_dollar10'] = p(t['import_price_index_pct_dollar10_path'], (3, 12, 24, 36))
    out['exports_pct_dollar10'] = p(t['exports_pct_dollar10_path'], (0, 12, 24, 36))
    out['import_volume_pct_dollar10'] = {k: p(v, (12, 24, 36)) for k, v in t['import_volume_pct_dollar10_path'].items()}
    out['sector_export_share_pct_gdp'] = {s: round(v, 2) for s, v in t['sector_export_share_pct_gdp'].items()}
    return out


if __name__ == '__main__':
    ap = argparse.ArgumentParser(); ap.add_argument('--write', action='store_true'); ap.add_argument('--json'); a = ap.parse_args()
    t = table()
    print(json.dumps(summary(t), indent=1))
    path = a.json or (paths.golden('trade_hand') if a.write else None)
    if path: json.dump(t, open(path, 'w'), indent=1); print('wrote', path)
