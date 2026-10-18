#!/usr/bin/env python3
"""Check the impulse-response bars of the re-fit spec (section 8G) against `econsim-cli irf`, `hirf` and `hvs` output.

Usage:  python3 scripts/irf_check.py [--stage {pre12,final}] FILE [FILE ...]
        Each FILE is text printed by `econsim-cli irf` (has "real GDP %"), `hirf` (has "foreclosures k"), or `hvs`
        (has "HVS rental vac"). Extra files are matched by content, so the order does not matter. Optional:
        --master-20k FILE (the master comparator at 20k x 16, `irf` format), which gives the H3 ratios (reported, not
        gated).

Each row prints PASS, FAIL, REPORT or TRACK. Status letters follow the spec: G = gated, G12 = housing bar (TRACK
in `--stage pre12`, gated from Step 12), R = report only. Exit status 1 if a gated row fails.

Statistics follow section 8G exactly. "Trough over mA-mB" is the minimum over the printed horizons in [A, B]; "peak"
is the maximum. Percent rows are the printed values (already shock / baseline - 1); pp rows are shock - baseline.
Only numpy and the stdlib are used.
"""
import argparse, re, sys

HZ_RE = re.compile(r'^\s*m\d+(\s+m\d+)+\s*$')
NUM_RE = re.compile(r'^[-+]?\d+(\.\d+)?$')
# Header suffix printed by the CLI: "=== <name>  (19.5s) ===". Only this trailing timing group is removed, so names
# that contain parentheses (for example "Gov purchases +5% (≈1% of GDP) x 24m") keep them.
TIMING_RE = re.compile(r'\s*\(\s*[\d.]+\s*(?:ms|µs|s|m)\s*\)\s*$')
EXTRA_RE = re.compile(r'^(dollar/policy ratio[^:]*|impact multiplier m3):\s*([-+]?\d+(\.\d+)?)\s*$')


def parse(path):
    """Return (kind, {section: {row: {horizon: value}}}, multipliers) for one output file."""
    text = open(path).read()
    if 'real GDP %' in text:
        kind = 'irf'
    elif 'HVS rental vac' in text:
        kind = 'hvs'
    elif 'foreclosures k' in text:
        kind = 'hirf'
    else:
        raise SystemExit(f'{path}: not irf, hirf or hvs output')
    sections, mult, sec, hz, extra = {}, {}, None, None, {}
    for line in text.splitlines():
        if line.startswith('==='):
            sec = re.sub(r'^=+\s*|\s*=+\s*$', '', line)
            sec = TIMING_RE.sub('', sec).strip()
            sections[sec] = {}
            hz = None
            continue
        e = EXTRA_RE.match(line.strip())
        if e and sec is not None:
            extra.setdefault(sec, {})[e.group(1)] = float(e.group(2))
            continue
        m = re.match(r'cumulative multiplier H=(\d+):\s*([-+]?\d+(\.\d+)?)', line.strip())
        if m and sec is not None:
            mult.setdefault(sec, {})[int(m.group(1))] = float(m.group(2))
            continue
        if HZ_RE.match(line):
            hz = [int(t[1:]) for t in line.split()]
            continue
        toks = line.split()
        if sec is None or hz is None or len(toks) < len(hz) + 1:
            continue
        vals = toks[-len(hz):]
        if not all(NUM_RE.match(v) for v in vals):
            continue
        label = ' '.join(toks[:-len(hz)])
        sections[sec][label] = dict(zip(hz, (float(v) for v in vals)))
    return kind, sections, mult, extra


GOV = 'Gov purchases +5% (≈1% of GDP) x 24m'


# Exact section names printed by `econsim-cli irf` and the housing probes (section 8G). A missing name is an error,
# so a renamed scenario cannot silently drop its rows: every name is looked up with need().
def need(sections, name, what):
    if name not in sections:
        raise SystemExit(f'{what}: required section "{name}" not found; sections: {sorted(sections)}')
    return sections[name]


def tr(rows, label, hs, fn=min):
    vals = [rows[label][h] for h in hs if h in rows[label]]
    if not vals:
        raise KeyError(f'{label} at {hs}')
    return fn(vals)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('files', nargs='+')
    ap.add_argument('--stage', choices=['pre12', 'final'], default='final')
    ap.add_argument('--master-20k')
    a = ap.parse_args()

    data = {}
    for f in a.files:
        kind, sec, mult, extra = parse(f)
        data.setdefault(kind, []).append((f, sec, mult, extra))
    if len(data.get('irf', [])) > 1 or len(data.get('hirf', [])) > 1 or len(data.get('hvs', [])) > 1:
        raise SystemExit('give one irf, one hirf and one hvs file')
    irf = data.get('irf', [(None, {}, {}, {})])[0]
    hirf = data.get('hirf', [(None, {}, {}, {})])[0]
    hvs = data.get('hvs', [(None, {}, {}, {})])[0]
    I, M, X = irf[1], irf[2], irf[3]
    H, V = hirf[1], hvs[1]
    pol = oil = tp = qe = heli = gov = hpol = hoil = htp = vpol = None
    if 'irf' in data:
        pol = need(I, 'Policy shock +100bp vs rule, 12m', 'irf')
        oil = need(I, 'Oil +50% (half-life 12m)', 'irf')
        tp = need(I, '10y term premium +100bp', 'irf')
        qe = need(I, 'QE 0.5% GDP/month x12 @10y', 'irf')
        heli = need(I, 'Helicopter $2,000 x 3m', 'irf')
        # The gov-spending scenario was added in Step 2c: pre12 may check a file that predates it. Final requires it.
        if a.stage == 'final' or GOV in I:
            gov = need(I, GOV, 'irf')
    if 'hirf' in data:
        hpol = need(H, 'Policy +100bp 12m', 'hirf')
        hoil = need(H, 'Oil +50%', 'hirf')
        htp = need(H, 'TP +100bp hl24', 'hirf')
    vtp = None
    if 'hvs' in data:
        vpol = need(V, 'Policy +100bp 12m', 'hvs')
        vtp = V.get('TP +100bp hl24')
    pol_x = X.get('Policy shock +100bp vs rule, 12m', {})
    rows = []  # (id, description, value text, status, band text, gated)

    def add(rid, desc, value, ok, band, status):
        """status G / G12 are gated (G12 is TRACK in pre12); R is report-only with a floor: a breach is FLOOR-FAIL."""
        if status == 'R':
            if ok is None:
                st = 'REPORT'
            else:
                st = 'REPORT' if ok else 'FLOOR-FAIL'
        elif ok is None:
            st = 'REPORT'
        elif ok:
            st = 'PASS'
        else:
            st = 'FAIL'
        if status == 'G12' and a.stage == 'pre12' and st == 'FAIL':
            st = 'TRACK'
        rows.append((rid, desc, value, st, band, status))

    H_ = [3, 6, 12, 24, 36]
    if pol is not None:
        gdp_tr = tr(pol, 'real GDP %', [3, 6, 12, 24])
        inv_tr = tr(pol, 'real inv %', [3, 6, 12, 24])
        add('P1', 'Policy +100bp: GDP trough m3-m24', f'{gdp_tr:.2f}', -1.6 <= gdp_tr <= -0.4, '-1.6..-0.4', 'G')
        ratio = inv_tr / gdp_tr if gdp_tr else float('nan')
        inv3 = pol['real inv %'][3]
        add('P2', 'Policy: inv trough / GDP trough (m3-m24)', f'{ratio:.2f}; inv m3 {inv3:.2f}',
            2.0 <= ratio <= 5.0 and inv3 <= -0.3, '2.0..5.0; inv m3 <= -0.3', 'G')
        u_pk = tr(pol, 'unemp pp', [3, 6, 12, 24], max)
        add('P3', 'Policy: u peak m3-m24', f'{u_pk:.2f}', 0.2 <= u_pk <= 1.0, '+0.2..+1.0', 'G')
        core_tr = tr(pol, 'core yoy pp', [6, 12, 24])
        add('P4', 'Policy: core trough m6-m24 (sign gate; magnitude R)', f'{core_tr:.2f}', core_tr <= -0.03,
            'sign <= -0.03', 'G')
        ten = pol['10y pp'][3]
        add('P5', 'Policy: 10y at m3', f'{ten:.2f}', 0.15 <= ten <= 0.45, '0.15..0.45', 'G')
        dol_pk = tr(pol, 'dollar %', [3, 6, 12], max)
        pol_pk = tr(pol, 'policy pp', [3, 6, 12], max)
        p6 = dol_pk / pol_pk if pol_pk else float('nan')
        add('P6', 'Policy: dollar peak / policy peak m3-m12', f'{p6:.2f}', p6 >= 1.2, '>= 1.2 (floor)', 'R')
        if 'dollar/policy ratio same-month m3' in pol_x:
            rows.append(('P6a', 'Policy: dollar / policy, same month m3 (reported)',
                         f"{pol_x['dollar/policy ratio same-month m3']:.2f}", 'REPORT', 'report', 'R'))
        if 'dollar/policy ratio max monthly m3-m6' in pol_x:
            rows.append(('P6b', 'Policy: dollar / policy, max monthly m3-m6 (reported)',
                         f"{pol_x['dollar/policy ratio max monthly m3-m6']:.2f}", 'REPORT', 'report', 'R'))
        gdp_ov = max(pol['real GDP %'][24], pol['real GDP %'][36])
        p7 = gdp_ov / abs(gdp_tr) if gdp_tr else float('nan')
        add('P7', 'Policy: GDP overshoot / |P1 trough|', f'{p7:.2f}', p7 <= 0.5, '<= 0.5', 'G')
        hpx = [pol['house px %'][h] for h in (12, 24, 36)]
        h1 = all(v <= -0.05 for v in hpx) and min(hpx) >= -5
        add('H1', 'Policy: house px m12/m24/m36 (irf 10k x 12)', ' / '.join(f'{v:.2f}' for v in hpx),
            h1, 'each <= -0.05; min >= -5', 'G12')
        add('H4', 'Policy: shelter yoy m24', f"{pol['shelter yoy pp'][24]:.2f}", pol['shelter yoy pp'][24] >= -0.05,
            '>= -0.05', 'G12')
    if hpol is not None:
        hpx = [hpol['house px %'][h] for h in (12, 24, 36)]
        add('H1b', 'Policy: house px m12/m24/m36 (hirf 20k x 16)', ' / '.join(f'{v:.2f}' for v in hpx),
            all(v <= 0 for v in hpx), 'each <= 0', 'G12')
    if vpol is not None:
        hv = vpol['HVS rental vac pp']
        mx = max(hv[h] for h in (3, 6, 12, 18, 24))
        mean3 = (hv[12] + hv[18] + hv[24]) / 3
        add('H5', 'Policy: HVS rental vacancy (max m3-m24; mean m12/m18/m24)', f'{mx:+.3f}; {mean3:+.3f}',
            mx <= 0.05 and mean3 <= 0.02, 'max <= +0.05; mean <= +0.02', 'G12')
        if 'shelter yoy pp' in vpol:
            rows.append(('H4v', 'Policy: HVS shelter yoy m24 (reported)', f"{vpol['shelter yoy pp'][24]:.3f}",
                         'REPORT', '-', 'R'))
        # b4-rent report-only rows (need `hvs ... flows`); no gate changes.
        if vtp is not None and 'HVS rental vac pp' in vtp:
            h5t = max(vtp['HVS rental vac pp'][h] for h in (3, 6, 12, 18, 24) if h in vtp['HVS rental vac pp'])
            add('H5t', 'TP: HVS rental vacancy max m3-m24 (reported)', f'{h5t:+.3f}', None, 'report', 'R')
        if 'renter->owner %' in vpol:
            add('H7', 'Policy: cumulative renter->owner % m24 (reported)', f"{vpol['renter->owner %'][24]:.2f}",
                None, 'report', 'R')
        if 'homeownership pp' in vpol:
            add('H8', 'Policy: homeownership pp m24 (reported)', f"{vpol['homeownership pp'][24]:.3f}", None,
                'report', 'R')
        if vtp is not None and 'hpi %' in vtp:
            h2i = min(vtp['hpi %'][h] for h in (12, 18, 24, 36) if h in vtp['hpi %'])
            add('H2i', 'TP: house price index trough m12-m36 (reported)', f'{h2i:.2f}', None, 'report', 'R')
    if oil is not None:
        o_gdp = tr(oil, 'real GDP %', [6, 12, 24])
        add('O1', 'Oil +50%: GDP trough m6-m24', f'{o_gdp:.2f}', -1.5 <= o_gdp <= -0.25, '-1.5..-0.25', 'G')
        o_cpi = tr(oil, 'CPI yoy pp', [3, 6, 12], max)
        add('O2', 'Oil: headline CPI peak m3-m12', f'{o_cpi:.2f}', 1.0 <= o_cpi <= 1.8, '1.0..1.8', 'G')
        o_core = tr(oil, 'core yoy pp', [3, 6, 12], max)
        add('O3', 'Oil: core peak m3-m12', f'{o_core:.2f}', o_core <= 0.50, '<= 0.50 (floor)', 'R')
        add('O4', 'Oil: durables at m12', f"{oil['durables %'][12]:.2f}", None, 'report (< 0 expected)', 'R')
    if oil is not None:
        h24, h36 = oil['house px %'][24], oil['house px %'][36]
        h3ok = -0.63 <= h24 <= 0 and -1.22 <= h36 <= 0
        add('H3', 'Oil: house px m24 / m36 (irf 10k x 12)', f'{h24:.2f} / {h36:.2f}', h3ok,
            'm24 in [-0.63,0]; m36 in [-1.22,0]', 'G12')
    if a.master_20k:
        # H3r (spec 8G H3): the 20k x 16 oil house-price response against the master. It is report-only, but a missing
        # hirf file or a missing master row is a failure, never a silent drop.
        # Stage 5 (tooling): the master may be an `irf` file or a `hirf` file of the same 20k x 16 design; the section names differ
        # ('Oil +50% (half-life 12m)' against 'Oil +50%'). Either is accepted; a missing section still fails.
        msec = parse(a.master_20k)[1]
        mname = 'Oil +50% (half-life 12m)' if 'Oil +50% (half-life 12m)' in msec else 'Oil +50%'
        mrow = need(msec, mname, 'master-20k')['house px %']
        if hoil is None:
            rows.append(('H3r', 'Oil house px vs master at 20k x 16, m24 / m36 (reported)',
                         'no hirf file', 'FAIL', 'needs hirf file', 'G'))
        else:
            hp = hoil['house px %']
            rows.append(('H3r', 'Oil house px vs master at 20k x 16, m24 / m36 (reported)',
                         f"{hp[24] / mrow[24]:.2f}x / {hp[36] / mrow[36]:.2f}x", 'REPORT', 'reported', 'R'))
    if tp is not None:
        t1 = tp['10y pp'][3]
        add('T1', 'TP +100bp: 10y at m3', f'{t1:.2f}', 0.85 <= t1 <= 1.0, '0.85..1.0', 'G')
        t2 = tr(tp, 'real GDP %', [6, 12, 24])
        add('T2', 'TP: GDP trough m6-m24', f'{t2:.2f}', -1.0 <= t2 <= -0.15, '-1.0..-0.15', 'G')
        ht = tr(tp, 'house px %', [12, 24, 36])
        add('H2', 'TP: house-price trough m12-m36', f'{ht:.2f}', -10 <= ht <= -5, '-10..-5', 'G12')
        t4 = tr(tp, 'sales %', [3, 6, 12])
        add('T4', 'TP: sales trough m3-m12', f'{t4:.2f}', t4 <= -7, '<= -7', 'G')
    if htp is not None:
        h2b = min(htp['house px %'][h] for h in (12, 24, 36))
        add('H2b', 'TP: house-price trough m12-m36 (hirf 20k x 16)', f'{h2b:.2f}', -10 <= h2b <= -5, '-10..-5', 'G12')
    if qe is not None:
        q1 = qe['10y pp'][12]
        add('Q1', 'QE: 10y at m12 (pp)', f'{q1:.2f}', -0.48 <= q1 <= -0.18, '-0.48..-0.18', 'G')
        q2 = qe['real GDP %'][12]
        add('Q2', 'QE: GDP at m12', f'{q2:.2f}', q2 >= 0, '>= 0', 'G')
    if heli is not None:
        x_g = heli['real GDP %'][3]
        x_d = heli['durables %'][3]
        x_c = tr(heli, 'core yoy pp', [3, 6, 12], max)
        add('X1', 'Helicopter: GDP m3 / durables m3 / core peak m3-m12', f'{x_g:.2f} / {x_d:.2f} / {x_c:.2f}',
            1 <= x_g <= 3 and x_d <= 20 and x_c <= 0.6, 'GDP 1..3; dur <= 20; core <= 0.6', 'G')
    if gov is not None:
        mm = M.get(GOV, {})
        m24, m12 = mm.get(24), mm.get(12)
        imp = X.get(GOV, {}).get('impact multiplier m3')
        if m24 is None or imp is None:
            raise SystemExit('F1: cumulative H24 and impact m3 multipliers are both required')
        add('F1', 'Gov purchases +5% x 24m: cumulative multiplier H24 (H12 in value text)',
            f'{m24:.2f} (H12 {m12:.2f})' if m12 is not None else f'{m24:.2f}', 0.5 <= m24 <= 1.5,
            '0.5..1.5', 'G')
        add('F1i', 'Gov purchases +5% x 24m: impact multiplier at m3 (dGDP / dGov, levels)', f'{imp:.2f}',
            0.4 <= imp <= 1.5, '0.4..1.5', 'G')
    print(f"{'id':5s} {'status':10s} {'value':34s} {'band':32s} description  [stage {a.stage}]")
    failed = []
    for rid, desc, value, st, band, status in rows:
        print(f"{rid:5s} {st:10s} {value:34s} {band:32s} {desc}")
        if (st == 'FAIL' and status in ('G', 'G12')) or st == 'FLOOR-FAIL':
            failed.append(rid)
    print(f"\n{len(rows)} rows; failing gated or floor rows: {failed or 'none'}")
    sys.exit(1 if failed else 0)


if __name__ == '__main__':
    main()
