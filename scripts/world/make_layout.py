#!/usr/bin/env python3
"""Build the stage-1 'repo' layout from the lead modeller's 'spec_work' working tree (the placement table of spec section 1.1; kept for reference, it only runs in that tree, which is not published):  python3 -B make_layout.py DEST
DEST becomes a repo-like tree:  scripts/world/*.py + pstar.json + targets.json + oil_reference.py, data/world/research/..., the golden directory.
The acceptance check of the placement is:  cd DEST && python3 -B -I scripts/world/world_ref.py selftest   (and `goldens --check`)."""
import os, sys, shutil
HERE = os.path.dirname(os.path.abspath(__file__)); sys.path.insert(0, HERE)
import paths
assert paths.LAYOUT == 'spec_work', 'run make_layout.py from the spec_work tree'

PY = ['paths.py', 'world_ref.py', 'bloc_ir.py', 'bloc_model.py', 'bloc_params.py', 'lin_pf.py', 're_statespace.py', 'world_net.py', 'us_shock_in2019.py',
      'us_shock_spillover.py', 'pin_tables.py', 'pf_monthly_net.py', 'exp_forcing.py', 'exp_forcing_check.py', 'trade_hand.py', 'oil_activity.py',
      'measure_ius_offset.py', 'make_layout.py']
BLOC_CSV = ['bloc_spillover_coeffs_2024.csv', 'bloc_trade_weights_2024.csv', 'discretisation_check.csv', 'own_shock_irfs_gpm.csv', 'state_space_eigs.csv']
OIL_CSV = ['eia_intl_annual_liquids_1990_2025.csv', 'steo_monthly_tidy.csv', 'oil_model_replay_2026.csv']


def cp(src, dst):
    os.makedirs(os.path.dirname(dst), exist_ok=True); shutil.copy2(src, dst)


def main(dest):
    sw = os.path.join(dest, 'scripts', 'world')
    for f in PY: cp(os.path.join(HERE, f), os.path.join(sw, f))
    cp(paths.PSTAR, os.path.join(sw, 'pstar.json')); cp(paths.TARGETS, os.path.join(sw, 'targets.json'))
    cp(paths.OIL_PROTOTYPE, os.path.join(sw, 'oil_reference.py'))
    rd = os.path.join(dest, 'data', 'world', 'research')
    for w in ('blocs', 'trade', 'oil', 'finance'): cp(paths.research_json(w), os.path.join(rd, w, 'params.json'))
    for f in BLOC_CSV: cp(paths.csv(f), os.path.join(rd, 'blocs', 'data', f))
    for f in OIL_CSV: cp(paths.oil_csv(f), os.path.join(rd, 'oil', 'data', f))
    gd = os.path.join(dest, 'crates', 'econsim-core', 'src', 'world', 'data', 'golden')
    for name, (src_name, dst_name) in paths.GOLDEN_FILES.items():
        s = os.path.join(paths.GOLDEN_DIR, src_name)
        if os.path.exists(s): cp(s, os.path.join(gd, dst_name))
    print('layout written to', dest)


if __name__ == '__main__':
    main(os.path.abspath(sys.argv[1]))
