#!/usr/bin/env python3
"""build_world_params.py [--out PATH] [--check]

Writes crates/econsim-core/src/world/data/world_params.json (spec section 1.7 and the schema of 1.9) from the four research
params.json files, the five bloc CSVs and scripts/world/pstar.json. Every number is read by path from the research files,
except the monthly forcing coefficients, which are the full-precision outputs of bloc_model.convert_to_monthly on the
quarterly table (the same values the offline solver uses), and the two monthly persistences 0.8^(1/3), which are computed
from the research quarterly values. Both are checked against the research monthly values (`check_monthly`).

--check: exit 0 iff (a) the calibration hash recomputed from the inputs equals meta.calibration_hash in the committed file and
(b) regenerating the file in memory gives the same JSON structure with every number within 1e-10 absolute plus 1e-10 relative.
"""
import os, sys, json, hashlib, argparse, math
HERE = os.path.dirname(os.path.abspath(__file__)); sys.path.insert(0, HERE)
import paths
from bloc_params import quarterly, BLOCS12
from bloc_model import convert_to_monthly

K, K_TAIL, T_KERNEL, T_PIN = 240, 480, 720, 480
BLOC_CSV_ORDER = ['bloc_spillover_coeffs_2024.csv', 'bloc_trade_weights_2024.csv', 'discretisation_check.csv',
                  'own_shock_irfs_gpm.csv', 'state_space_eigs.csv']
OUT_DEFAULT = os.path.join(paths.ROOT, 'crates', 'econsim-core', 'src', 'world', 'data', 'world_params.json') if paths.LAYOUT == 'repo' else None
PLACE = {'EA': 'euro_area', 'CN': 'China', 'JP': 'Japan', 'UK': 'UK', 'CA': 'Canada', 'MX': 'Mexico', 'IN': 'India', 'KR': 'Korea',
         'BR': 'Brazil', 'AU': 'Australia', 'OPEC+': 'OPEC+', 'RoW': 'rest_of_world'}


def canonical(obj):
    return json.dumps(obj, sort_keys=True, separators=(',', ':'))


def calibration_hash():
    """sha256 of: the four research params.json (canonical JSON, order blocs, trade, oil, finance), pstar.json (canonical), the five
    bloc CSVs (raw bytes, placement order), and the string K=240;KT=480;T=720;TP=480;  (spec 1.9)."""
    h = hashlib.sha256()
    for w in ('blocs', 'trade', 'oil', 'finance'):
        h.update(canonical(json.load(open(paths.research_json(w)))).encode('utf-8'))
    h.update(canonical(json.load(open(paths.PSTAR))).encode('utf-8'))
    for f in BLOC_CSV_ORDER:
        h.update(open(paths.csv(f), 'rb').read())
    h.update(b'K=240;KT=480;T=720;TP=480;')
    return h.hexdigest()


def val(node):
    return node['value'] if isinstance(node, dict) and 'value' in node else node


def load_research():
    return {w: json.load(open(paths.research_json(w))) for w in ('blocs', 'trade', 'oil', 'finance')}


def check_monthly(name, m, rb):
    """The monthly coefficients must equal the research monthly_coefficients (5-digit values) within the selftest tolerance."""
    r = rb[name]['monthly_coefficients']
    for k in ('b1', 'b2', 'b3', 'b4', 'b5', 'l1', 'l2', 'l3', 'g1', 'g2', 'g4', 'phi', 'theta'):
        rv = val(r.get(k))
        if rv is None:
            continue
        if abs(m[k] - rv) > 5e-5 + 1e-4 * abs(rv):
            raise SystemExit('monthly coefficient %s %s: converted %r, research %r' % (name, k, m[k], rv))


def build():
    rs = load_research()
    bp = rs['blocs']                 # research blocs/params.json: keys meta, global, blocs
    rb = bp['blocs']
    pstar = json.load(open(paths.PSTAR))
    import world_ref as wr
    S_US, W_US = wr.S_US, wr.W_US
    S_BB, W_BB = wr.S_BB, wr.W_BB
    blocs = []
    for b in BLOCS12:
        e = rb[b]
        q = dict(quarterly(b)); q.setdefault('phi', 0.8)
        m = convert_to_monthly(q)
        check_monthly(b, m, rb)
        p = pstar[b]
        uip_w = q['uip_w'] if p['uip_w'] == 'research' else float(p['uip_w'])
        monthly = {k: float(m[k]) for k in ('b1', 'b2', 'b3', 'b4', 'b5', 'l1', 'l2', 'l3', 'g1', 'g2', 'g4', 'phi')}
        monthly['uip_w'] = float(q['uip_w'])
        monthly['theta'] = float(m['theta'])
        ss = e['steady_state']
        okun = e.get('okun')
        oil = e['oil']
        ob = rs['oil']['blocs'][PLACE[b]]['crude_share_of_fuel_price_phi']
        blocs.append({
            'name': b,
            'inflation_target': float(val(ss['inflation_target'])),
            'neutral_real_rate': float(val(ss['neutral_real_rate'])),
            'policy_rate_floor': float(val(ss['policy_rate_floor'])),
            'monthly': monthly,
            'pstar': {'theta_scale': float(p['theta_scale']), 'lam_rp': float(p['lam_rp']), 'k_us': float(p['k_us']),
                      'g_z': float(p['g_z']), 'uip_w': float(uip_w), 'oil_is_scale': float(p['oil_is_scale'])},
            'sd': {'demand': float(val(e['shocks_monthly_sd']['sd_demand'])), 'cost_push': float(val(e['shocks_monthly_sd']['sd_cost_push'])),
                   'policy': float(val(e['shocks_monthly_sd']['sd_policy'])), 'risk_premium': float(val(e['shocks_monthly_sd']['sd_risk_premium']))},
            'okun': None if okun is None else {'a1': float(val(okun['alpha1_lag_monthly'])), 'a2': float(val(okun['alpha2_gap_monthly']))},
            'oil': {'is_coef': float(val(oil['IS_oil_coefficient_monthly'])), 'energy_cpi_weight': float(val(oil['energy_cpi_weight'])),
                    'phi_fuel': float(val(ob))},
            'weight_ppp': float(val(e['weights']['share_world_pct_ppp_2024'])) / 100.0,
            'weight_usd': float(val(e['weights']['share_world_pct_usd_2024'])) / 100.0,
        })
    # Rest-of-world and OPEC+ GDP weights under both OPEC+ composition sets (spec D7). UAE GDP = with-UAE total minus ex-UAE total.
    comp = bp['global']['composition_notes']['composites']['opecplus']
    uae_usd = comp['with_UAE']['gdp_usd_bn_2024']['total_bn'] - comp['ex_UAE_oct2026']['gdp_usd_bn_2024']['total_bn']
    uae_ppp = comp['with_UAE']['gdp_ppp_bn_2024']['total_bn'] - comp['ex_UAE_oct2026']['gdp_ppp_bn_2024']['total_bn']
    idx = {b['name']: i for i, b in enumerate(blocs)}
    world_usd = rb['OPEC+']['weights']['gdp_usd_bn_2024']['value'] / (rb['OPEC+']['weights']['share_world_pct_usd_2024']['value'] / 100.0)
    world_ppp = rb['OPEC+']['weights']['gdp_ppp_bn_2024']['value'] / (rb['OPEC+']['weights']['share_world_pct_ppp_2024']['value'] / 100.0)
    uae_in = {'weight_usd': [b['weight_usd'] for b in blocs], 'weight_ppp': [b['weight_ppp'] for b in blocs]}
    uae_out = {'weight_usd': list(uae_in['weight_usd']), 'weight_ppp': list(uae_in['weight_ppp'])}
    o, r = idx['OPEC+'], idx['RoW']
    # Ex-UAE OPEC+ is the with-UAE weight less the UAE transfer, so the 12-bloc total is conserved exactly (the research
    # totals are rounded to 0.1 bn; ex_UAE_oct2026.total_bn agrees with 6661.3 - 552.3 to that rounding).
    uae_out['weight_usd'][o] = uae_in['weight_usd'][o] - uae_usd / world_usd
    uae_out['weight_usd'][r] = uae_in['weight_usd'][r] + uae_usd / world_usd
    uae_out['weight_ppp'][o] = uae_in['weight_ppp'][o] - uae_ppp / world_ppp
    uae_out['weight_ppp'][r] = uae_in['weight_ppp'][r] + uae_ppp / world_ppp
    uae_in = {k: [float(x) for x in v] for k, v in uae_in.items()}
    uae_out = {k: [float(x) for x in v] for k, v in uae_out.items()}
    fin = bp['global']['financial_channel']
    rho_g_q = float(val(fin['rho_G']))
    rho_rp_q = float(val(rb['EA']['FX']['risk_premium_shock_AR_quarterly']))
    trade = build_trade(rs)
    out = {
        'meta': {'calibration_hash': calibration_hash(), 'k': K, 'k_tail': K_TAIL, 't_kernel': T_KERNEL, 't_pin': T_PIN,
                 'blocs': list(BLOCS12), 'pstar_sha': wr.pstar_sha(),
                 'note': 'monthly from bloc_model.convert_to_monthly(quarterly); persistences 0.8^(1/3); see build_world_params.py'},
        'blocs': blocs,
        'network': {'s_us': [float(x) for x in S_US], 's_bb': [[float(x) for x in row] for row in S_BB],
                    'w_us': [float(x) for x in W_US], 'w_bb': [[float(x) for x in row] for row in W_BB]},
        'financial': {'rho_g': float(rho_g_q ** (1.0 / 3.0)), 'rho_rp': float(rho_rp_q ** (1.0 / 3.0))},
        'opec': {'uae_in': uae_in, 'uae_out': uae_out, 'uae_gdp_usd_bn_2024': float(uae_usd), 'uae_gdp_ppp_bn_2024': float(uae_ppp)},
        'trade': trade,
        'oil': build_oil(rs),
        'finance': build_finance(rs),
    }
    return out


def build_finance(rs):
    """Stage 4-1 (spec 1.7, 2.6): the risk-off loadings of the dollar by bloc, % dollar appreciation per VIX point, copied by path from
    finance dollar.risk_off_loading_gamma_pct_per_vix_pt.by_bloc (bloc order = H10_KEYS, the solution order of the blocs)."""
    fp = rs['finance']
    bl = fp['dollar']['risk_off_loading_gamma_pct_per_vix_pt']['by_bloc']
    out = {'gamma_pct_per_vix': [float(val(bl[k])) for k in H10_KEYS]}
    # Stage 4-2 (spec 1.7, 2.7): the official tranche tables, copied by path from finance treasury_demand (bloc order = H10_KEYS):
    # TIC-recorded holdings by bloc ($bn), the official share of each bloc's holdings, and the sourced official total ($bn).
    td = fp['treasury_demand']
    out['tic_ust_bn'] = [float(val(td['foreign_holdings_ust_bn_2026_07']['by_bloc'][k])) for k in H10_KEYS]
    out['official_share'] = [float(val(td['official_share_of_bloc_ust_holdings']['by_bloc'][k])) for k in H10_KEYS]
    out['foreign_official_ust_bn'] = float(val(td['foreign_official_ust_bn']))
    out['positions'] = build_positions(rs)
    return out


# Stage 4-3 (spec 2.8, D22, D40): the memo position tables, copied by path from finance `cross_border` (bloc order = H10_KEYS), real-world
# $bn. The two ledger stocks carry only the bloc shares (normalised): Treasuries by the TIC holdings, deposits by the BIS liabilities.
# FC_BANK is the foreign-currency share of US bank claims [assumed, not in the research].
FC_BANK = 0.3


def build_positions(rs):
    cb = rs['finance']['cross_border']
    bl = lambda d: [float(val(d['by_bloc'][k])) for k in H10_KEYS]
    sh = cb['foreign_holdings_of_us_securities_bn_2025_06']
    ho = cb['us_holdings_of_foreign_securities_bn_2025_12']
    mtc = cb['market_to_historical_cost_ratio_fdi']
    fc = cb['foreign_currency_share']
    yl = cb['implied_annual_yield_on_iip_positions_pct']
    tic = [float(val(rs['finance']['treasury_demand']['foreign_holdings_ust_bn_2026_07']['by_bloc'][k])) for k in H10_KEYS]
    dep = bl(cb['us_bank_cross_border_liabilities_bn_2026q1'])
    ag, co, st = bl(sh['lt_agency']), bl(sh['lt_corporate']), bl(sh['st_debt'])
    eq_load = rs['finance']['global_financial_cycle']['equity_loading_relative_to_us']
    return {
        'a_eq': bl(ho['equity']),
        'a_debt': [a + b for a, b in zip(bl(ho['lt_debt']), bl(ho['st_debt']))],
        'a_fdi': [x * float(val(mtc['assets'])) for x in bl(cb['us_direct_investment_abroad_bn_2025_hist_cost'])],
        'a_bank': bl(cb['us_bank_cross_border_claims_bn_2026q1']),
        'l_agcorp': [a + b + c for a, b, c in zip(ag, co, st)],
        'l_eq': bl(sh['equities']),
        'l_fdi': [x * float(val(mtc['liabilities'])) for x in bl(cb['fdi_in_united_states_bn_2025_hist_cost'])],
        'tic_share': [x / sum(tic) for x in tic],
        'dep_share': [x / sum(dep) for x in dep],
        'equity_loading': [float(val(eq_load['by_bloc'][k])) for k in H10_KEYS],
        'fc_eq': float(val(fc['us_foreign_equity_foreign_currency'])),
        'fc_debt': 1.0 - float(val(fc['us_foreign_debt_securities_usd_denominated'])),
        'fc_fdi': float(val(fc['us_fdi_abroad_foreign_currency'])),
        'fc_bank': FC_BANK,
        'fc_liab': float(val(fc['us_liabilities_foreign_currency'])),
        'yield_assets_pct': float(val(yl['assets_4q_avg_2025q3_2026q2'])),
        'yield_liab_pct': float(val(yl['liabilities_4q_avg'])),
    }


# Sector order of the trade matrices (trade params sector_shares: Energy ... Construction) = the model's Sector order.
TRADE_SECTORS = ['Energy', 'Food', 'Durables', 'Nondurables', 'Tech', 'Leisure', 'HealthEdu', 'OtherServices', 'Construction']
# US 2024 GDP (bn USD) used by the spec for the export base shares (section 2.5, D14: "over the 2024 GDP ($29,381bn)", sum 10.96%).
# The research openness table gives 29,298 for the same year; the spec figure is used and the difference is an open item.
GDP_2024_BN = 29381.0
# Bloc keys of the H10 broad dollar table (finance dollar.h10_broad_weights_pct_2026.by_bloc), in BLOCS12 order.
H10_KEYS = ['euro_area', 'china', 'japan', 'uk', 'canada', 'mexico', 'india', 'korea', 'brazil', 'australia', 'opec_plus', 'rest_of_world']


# Stage 3 (spec 2.4, 1.7): the oil market parameters, copied by path from oil/params.json (the research values; the profiles
# are the alternatives' `changes` dictionaries under their research keys). Regions: US first, then the twelve blocs in solution
# order (EA, CN, JP, UK, CA, MX, IN, KR, BR, AU, OPEC+, RoW). Consumption shares omega_i, income elasticities beta_i and crude
# shares phi_i are the research's blocs.<name> columns; eta0_i = eta0 phi_i / phibar and eta1_i likewise (D12) are computed in Rust.
OIL_REGIONS = [('US', 'US'), ('EA', 'euro_area'), ('CN', 'China'), ('JP', 'Japan'), ('UK', 'UK'), ('CA', 'Canada'), ('MX', 'Mexico'),
               ('IN', 'India'), ('KR', 'Korea'), ('BR', 'Brazil'), ('AU', 'Australia'), ('OPEC+', 'OPEC+'), ('RoW', 'rest_of_world')]
OIL_PROFILES = [('default', None), ('supply_heavy', 'supply_heavy'), ('low_demand', 'low_demand_elasticity'),
                ('high_demand', 'high_demand_elasticity'), ('shale_boom', 'shale_boom'), ('shale_discipline', 'shale_discipline')]
OPEC_UAE_SPARE_SHARE = 0.30   # [assumed] UAE share of OPEC+ spare capacity (spec D7; a runtime switch in Rust)


def uae_liquids_2024_mbd():
    """UAE petroleum and other liquids production 2024 (EIA INTL 53-1, thousand b/d converted to mb/d)."""
    import csv
    path = os.path.join(paths.OIL_DATA_DIR, 'eia_intl_annual_liquids_1990_2025.csv')
    with open(path) as f:
        for row in csv.DictReader(f):
            if row['series_id'] == 'INTL.53-1-ARE-TBPD.A' and row['year'] == '2024':
                return float(row['value_kbd']) / 1000.0
    raise SystemExit('UAE 2024 liquids not found in ' + path)


def build_oil(rs):
    o = rs['oil']
    bl = o['blocs']
    regions = []
    for key, place in OIL_REGIONS:
        b = bl[place]
        regions.append({'name': key, 'omega': float(val(b['consumption_share'])), 'beta': float(val(b['cyclical_income_elasticity'])),
                        'phi': float(val(b['crude_share_of_fuel_price_phi']))})
    omega_sum = sum(r['omega'] for r in regions)
    if abs(omega_sum - 1.0) > 5e-4:
        raise SystemExit('oil consumption shares sum to %r' % omega_sum)
    phibar = sum(r['omega'] * r['phi'] for r in regions)
    dem = o['demand_aggregate']
    sup = o['supply']
    inv = o['inventories']
    sh = o['shocks']
    prof = {}
    for name, key in OIL_PROFILES:
        if key is None:
            prof[name] = {}
        else:
            prof[name] = {k: float(v) for k, v in o['alternatives'][key]['changes'].items()}
    opec_liq = float(val(bl['OPEC+']['liquids_production_2024_mbd']))
    uae_liq = uae_liquids_2024_mbd()
    return {
        'q_mbd': float(o['_meta']['Q_mbd']),
        'brent_usd': float(val(o['anchors']['Brent_2024_usd'])),
        'stochastic_mean_offset': float(val(o['anchors']['stochastic_mean_offset'])),
        'w': float(val(o['time_aggregation']['w'])),
        'eta0': float(val(dem['eta0'])), 'eta1': float(val(dem['eta1'])), 'rho_z': float(val(dem['rho_z'])),
        'rho_ylag': float(val(dem['rho_ylag'])), 'beta_y': float(val(dem['beta_y'])),
        'phibar': phibar,
        'regions': regions,
        'supply': {
            'eps_s': float(val(sup['eps_S'])), 'spare_cap': float(val(sup['opec']['spare_capacity'])),
            'theta_o': float(val(sup['opec']['theta_O'])), 'pi_l': float(val(sup['opec']['price_floor'])),
            'pi_h': float(val(sup['opec']['price_ceiling'])), 'kap_l': float(val(sup['opec']['kappa_L'])),
            'kap_h': float(val(sup['opec']['kappa_H'])), 'tau_min': float(val(sup['opec']['tau_min'])),
            'tau_decay': float(val(sup['opec']['tau_decay'])), 'stock_kappa': 0.0,
            'market_share_unwind': float(val(sup['opec']['market_share_unwind_rate'])),
            'sh_us': float(val(sup['us_tight_oil']['share_of_Q'])), 'eps_us_lr': float(val(sup['us_tight_oil']['eps_LR'])),
            'rho_us': float(val(sup['us_tight_oil']['rho_stage'])),
            'sh_rest': float(val(sup['rest_of_world_conventional']['share_of_Q'])),
            'eps_rest_lr': float(val(sup['rest_of_world_conventional']['eps_LR'])),
            'rho_rest': float(val(sup['rest_of_world_conventional']['rho'])),
        },
        'inventories': {'psi': float(val(inv['psi'])), 'rho_e': float(val(inv['rho_E'])), 'xi': float(val(inv['xi_builds'])),
                        'xi_d': float(val(inv['xi_draws'])), 'lam_n': float(val(inv['lambda_news']))},
        'shocks': {
            'rho_i': float(val(sh['oil_specific']['rho_I'])), 'rho_q': float(val(sh['oil_specific']['rho_q'])),
            'theta_q': float(val(sh['oil_specific']['theta_q'])), 'sig_i': float(val(sh['oil_specific']['sigma'])),
            'nu_i': float(val(sh['oil_specific']['student_nu'])), 'garch_a': float(val(sh['oil_specific']['garch']['alpha'])),
            'garch_b': float(val(sh['oil_specific']['garch']['beta'])),
            'jump_lam_i': float(val(sh['oil_specific']['jumps']['intensity'])),
            'jump_mu_i': float(val(sh['oil_specific']['jumps']['mean_sd_units'])),
            'jump_sd_i': float(val(sh['oil_specific']['jumps']['sd_sd_units'])),
            'rho_d': float(val(sh['supply_disruption']['rho_D'])), 'jump_lam_d': float(val(sh['supply_disruption']['intensity'])),
            'jump_mean_d': float(val(sh['supply_disruption']['mean_size'])),
            'sig_o': float(val(sh['opec_policy_surprise']['sigma'])), 'nu_o': float(val(sh['opec_policy_surprise']['student_nu'])),
            'sig_y': float(val(sh['activity']['sigma'])), 'rho_y': float(val(sh['activity']['rho_y'])),
            'news_cut_share': float(val(sh['news']['cut_share'])), 'news_phase': float(val(sh['news']['phase'])),
            'rho_n': float(val(sh['news']['rho_n'])),
        },
        'opec': {'liquids_mbd': opec_liq, 'uae_liquids_mbd': uae_liq, 'uae_spare_share': OPEC_UAE_SPARE_SHARE,
                 'uae_in_liquids_mbd': opec_liq, 'uae_out_liquids_mbd': opec_liq - uae_liq},
        'profiles': prof,
    }


def build_trade(rs):
    """Stage 2 tables (spec 1.7, 2.5): every number is read by path from the trade and finance research files. Shares and
    values are stored as the research has them; the normalisation of the H10 weights (sum 100.01 in the table) is recorded
    as `dollar_weights_sum_raw`."""
    tp = rs['trade']
    fp = rs['finance']
    imp = tp['sector_shares']['matrices']['imports_base_C26_in_Tech']['usd_bn_2024']
    exp = tp['sector_shares']['matrices']['exports_base_C26_in_Tech']['usd_bn_2024']
    gm = tp['bloc_goods_matrix']['import_shares_2024']
    theta = [float(val(tp['invoicing']['theta_usd_share_of_bloc_exports_to_US'][b])) for b in BLOCS12]
    imports_base = [[float(imp[b][s]) for s in TRADE_SECTORS] for b in BLOCS12]
    exports_base = [[float(exp[b][s]) for s in TRADE_SECTORS] for b in BLOCS12]
    import_shares = [[float(gm[k][j]) for j in BLOCS12] for k in BLOCS12]
    h10 = fp['dollar']['h10_broad_weights_pct_2026']['by_bloc']
    raw = [float(val(h10[k])) / 100.0 for k in H10_KEYS]
    dollar_weights = [x / sum(raw) for x in raw]
    out = {
        'gdp_2024_bn': GDP_2024_BN,
        'sectors': TRADE_SECTORS,
        'theta': theta,
        'imports_base': imports_base,
        'exports_base': exports_base,
        'import_shares': import_shares,
        'dollar_weights': dollar_weights,
        'dollar_weights_sum_raw': sum(raw),
        'dcp_kappa_usd': float(val(tp['pass_through']['us_import_price_from_bloc_formula']['G_usd_long_run'])),
        'dcp_kappa_nonusd': float(val(tp['pass_through']['us_import_price_from_bloc_formula']['G_nonusd_long_run'])),
        'export_fx_sr': -float(val(tp['pass_through']['dominant_currency_volume_response']['bilateral_volume_elasticity_to_importer_usd_rate_impact'])),
        'foreign_income_elasticity': float(val(tp['elasticities']['us_exports']['foreign_income_elasticity'])),
        'origin_substitution_lr': float(val(tp['elasticities']['us_imports']['origin_substitution_elasticity'])),
    }
    out['retaliation'] = build_retaliation(rs)
    return out


# Stage 4-3 (spec 2.5, D9): the retaliation rule tables, copied by path from trade `retaliation.recommended_rule`. The research names
# the euro area "EU" in these tables; blocs without a lag entry (JP, KR, UK, BR, AU, OPEC+, RoW) get RET_LAG_DEFAULT months [assumed]
# (only RoW has kappa > 0 among them); the cap of every bloc other than CN is `cap.others`.
RET_ALIAS = {'EA': 'EU'}
RET_LAG_DEFAULT = 1


def build_retaliation(rs):
    rr = rs['trade']['retaliation']['recommended_rule']
    key = lambda b: RET_ALIAS.get(b, b)
    kappa = [float(val(rr['kappa'][key(b)])) for b in BLOCS12]
    lag = [int(val(rr['lag_months'][key(b)])) if key(b) in rr['lag_months'] else RET_LAG_DEFAULT for b in BLOCS12]
    cap = [float(val(rr['cap'][b])) if b in rr['cap'] else float(val(rr['cap']['others'])) for b in BLOCS12]
    w_s = [float(val(rr['sector_weights_w_s'][s])) for s in TRADE_SECTORS]
    return {'kappa': kappa, 'lag': lag, 'cap': cap, 'w_s': w_s}


def compare(a, b, path='', tol=1e-10):
    """Structure must match; numbers within tol absolute plus tol relative. Returns the maximum absolute difference."""
    if isinstance(a, dict):
        if a.keys() != b.keys():
            raise AssertionError('keys differ at %s: %s' % (path, sorted(set(a) ^ set(b))))
        return max([compare(a[k], b[k], path + '/' + k, tol) for k in a] + [0.0])
    if isinstance(a, list):
        if len(a) != len(b):
            raise AssertionError('length differs at %s' % path)
        return max([compare(x, y, path, tol) for x, y in zip(a, b)] + [0.0])
    if isinstance(a, (int, float)) and not isinstance(a, bool):
        d = abs(a - b)
        if d > tol + tol * abs(b):
            raise AssertionError('value differs at %s: %r vs %r' % (path, a, b))
        return d
    if a != b:
        raise AssertionError('value differs at %s: %r vs %r' % (path, a, b))
    return 0.0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--out', default=OUT_DEFAULT)
    ap.add_argument('--check', action='store_true')
    a = ap.parse_args()
    if a.out is None:
        raise SystemExit('no default output path in this layout; pass --out')
    doc = build()
    if a.check:
        committed = json.load(open(a.out))
        if committed['meta']['calibration_hash'] != doc['meta']['calibration_hash']:
            print('calibration_hash differs from the committed file'); sys.exit(1)
        d = compare(json.loads(json.dumps(doc)), committed)
        print('build_world_params --check OK: hash equal; max abs difference %.3e' % d)
        return
    os.makedirs(os.path.dirname(a.out), exist_ok=True)
    with open(a.out, 'w') as f:
        json.dump(doc, f, indent=1)
        f.write('\n')
    print('wrote', a.out, 'hash', doc['meta']['calibration_hash'][:16])


if __name__ == '__main__':
    main()
