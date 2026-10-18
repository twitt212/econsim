#!/usr/bin/env python3
"""world_ius_offset (spec D-R1): the mean, over months 0-239 and over seeds, of  policy_rate - (r_star_est + pi_lr)  in pp.
These are the ABM's own monthly output fields (all in percent): policy_rate = the policy rate, r_star_est = current_r_star(w), pi_lr = long-run inflation
estimate, so  r_star_est + pi_lr = i_inf  (the neutral nominal rate the spec uses in ius_t = 100 (i_us - i_inf) - world_ius_offset).
A fixed config constant, measured ONCE on default runs of the base binary (world block off); it is not re-estimated per run.

Usage:  python3 -B measure_ius_offset.py --files S/ius0/s*.json [--seeds 12]     (files written by  econsim-cli run 20000 240 SEED --json | tail -n 1 )
        python3 -B measure_ius_offset.py --runs DIR --tag s1_S6 --seeds 12         (gz run files  DIR/TAG_s<seed>.json.gz)
Prints the mean over the first N seeds, the mean over all given seeds, and the across-seed sd.
"""
import sys, json, gzip, glob, argparse
import numpy as np

def load(path):
    f = gzip.open(path, 'rt') if path.endswith('.gz') else open(path)
    txt = f.read().strip().splitlines()[-1]
    return json.loads(txt)

def dev(path):
    d = load(path)
    pr = np.array([r['policy_rate'] for r in d]); rs = np.array([r['r_star_est'] for r in d]); pl = np.array([r['pi_lr'] for r in d])
    assert len(d) == 240, 'expected 240 reported months, got %d' % len(d)
    return float((pr - (rs + pl)).mean())

if __name__ == '__main__':
    ap = argparse.ArgumentParser(); ap.add_argument('--files', nargs='*'); ap.add_argument('--runs'); ap.add_argument('--tag', default='s1_S6'); ap.add_argument('--seeds', type=int, default=12)
    a = ap.parse_args()
    files = a.files or sorted(glob.glob('%s/%s_s*.json.gz' % (a.runs, a.tag)), key=lambda p: int(p.rsplit('_s', 1)[1].split('.')[0]))
    v = np.array([dev(f) for f in files])
    print('seeds', len(v), ' per-seed means (pp):', np.round(v, 2).tolist())
    print('mean over first %d seeds: %.3f pp   (standard error %.3f)' % (min(a.seeds, len(v)), v[:a.seeds].mean(), v[:a.seeds].std(ddof=1) / np.sqrt(min(a.seeds, len(v)))))
    print('mean over all %d seeds:   %.3f pp   across-seed sd %.3f' % (len(v), v.mean(), v.std(ddof=1)))
