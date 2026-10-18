#!/usr/bin/env python3
"""Labour-market duration facts (b5-udur): model unemployment duration, its cyclicality and the finding ratio.

Usage:  facts_labour.py CLI            (the scorecard's dimension contract; see stylized_facts.py)
Env:    FACTS_RESULT_JSON  path to write {"facts": [{id, name, target, lo, hi, model, gap, sample, source}]}
        FACTS_SEEDS (8), runs through facts_cycle.run_model, so the shared run cache serves them.

Duration conventions follow scripts/labour_check.py and paired.py: months 12..239, mean weeks = (mean months + 0.5) x
4.345, median = grouped median x 4.345, LTU = 6+ months unemployed. US values: BLS CPS via FRED (UEMPMEAN, UEMPMED,
UEMP27OV/UNEMPLOY) and the 19-year window medians of the same series (see docs/CALIBRATION.md, b5-udur section).
Only numpy and the stdlib are used, plus paired and facts_cycle from this directory.
"""
import json, os, sys
from concurrent.futures import ThreadPoolExecutor
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import facts_cycle as fc  # noqa: E402
import paired  # noqa: E402

CLI = sys.argv[1] if len(sys.argv) > 1 else os.environ.get('ECONSIM_CLI', os.path.join(HERE, '..', 'target', 'release', 'econsim-cli'))
SEEDS = int(os.environ.get('FACTS_SEEDS', 8))
HH = int(os.environ.get('FACTS_HH', 20000))
MONTHS = 240
JOBS = int(os.environ.get('FACTS_JOBS', 8))
CACHE = os.environ.get('FACTS_CACHE')


def rows_from(recs):
    h = recs[12:240]
    u = np.array([m['unemployment_rate'] for m in h], float)
    # BLS clock when the records carry it (b6-reentry keys), else the resumed clock
    bls = all(k in h[0] for k in paired.BLS_KEYS)
    keys = paired.BLS_KEYS if bls else paired.RESUMED_KEYS
    st = paired.dur_stats(h, u, keys=keys)
    ltu = np.array([m.get(keys[2], np.nan) for m in h], float)
    peak = float(np.nanmax(np.convolve(ltu, np.ones(3) / 3, mode='valid')))
    st.update(mean=float(np.nanmean([m.get(keys[0], np.nan) for m in h])),
              med=float(np.nanmean([m.get(keys[1], np.nan) for m in h])), peak=peak)
    return st


def main():
    with ThreadPoolExecutor(JOBS) as ex:
        runs = list(ex.map(lambda s: fc.run_model(CLI, HH, MONTHS, s, CACHE), range(1, SEEDS + 1)))
    per = [rows_from(r) for r in runs]
    mean = lambda k: float(np.nanmean([p[k] for p in per]))
    dur_ratio = mean('mean') / mean('med') if mean('med') else float('nan')
    jf = paired.jf_ratio([{'jf_h': p['jf_h'], 'jf_s': p['jf_s']} for p in per])
    rows = [
        ('dur_mean', 'mean unemployment duration, weeks', 16.0, 21.0, 17.7, mean('mean'), 'UEMPMEAN 1967-2019 17.7; 1985-2019 20.3'),
        ('dur_median', 'median unemployment duration, weeks', 8.0, 11.0, 8.7, mean('med'), 'UEMPMED 1967-2019 8.7; 1985-2019 9.8'),
        ('ltu_share', 'long-term unemployed (27+ weeks), % of unemployed', 15.0, 23.0, 18.0, mean('ltu'),
         'UEMP27OV/UNEMPLOY 1967-2019 average'),
        ('ltu_peak', 'LTU share, 3-month average peak per seed, %', 26.0, 46.0, 30.0,
         float(np.nanmean([p['peak'] for p in per])), '19-year windows: median 25.0, p90 45.1 (UEMP27OV/UNEMPLOY)'),
        ('dur_slope', 'slope of median duration (weeks) on u, per pp', 1.2, 2.5, 1.29, mean('dur_slope'),
         'FRED UEMPMED on UNRATE, 1967-2019 1.29; 1985-2019 1.93'),
        ('ltu_lag', 'LTU share lag behind u, months (argmax corr)', 6.0, 12.0, 8.0, mean('ltu_lag'),
         'FRED UEMP27OV/UNEMPLOY vs UNRATE: 8 (1967-2019), 10 (1985-2019)'),
        ('med_lag', 'median duration lag behind u, months (argmax corr)', 3.0, 8.0, 6.0, mean('med_lag'),
         'FRED UEMPMED vs UNRATE: 6 (1967-2019), 8 (1985-2019)'),
        ('dur_ratio', 'mean ÷ median duration', 1.7, 2.3, 2.03, dur_ratio, 'FRED UEMPMEAN ÷ UEMPMED, 1967-2019'),
        ('jf_lt_st', 'job finding 13+ months ÷ 1-3 months (pooled)', 0.33, 0.42, 0.375, jf,
         'Krueger, Cramer & Cho (2014); Kroft, Lange, Notowidigdo & Katz (2016)'),
    ]
    facts = []
    for key, name, lo, hi, point, val, src in rows:
        t = dict(lo=lo, hi=hi, point=point)
        gap = fc.flag(t, val)
        facts.append(dict(id=key, name=name, target=f'{lo:g}-{hi:g}', lo=lo, hi=hi,
                          model=None if val is None or not np.isfinite(val) else round(float(val), 4),
                          gap=gap, sample=f'{SEEDS} seeds x {HH} x {MONTHS}, months 12-239', source=src))
    out = os.environ.get('FACTS_RESULT_JSON')
    if out:
        with open(out, 'w') as f:
            json.dump({'facts': facts}, f, indent=1)
    for f_ in facts:
        print(f"{f_['id']:12s} model {f_['model']!s:>10} band {f_['target']:>10} {f_['gap']}")


if __name__ == '__main__':
    main()
