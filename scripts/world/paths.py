"""Every file location used by the reference tooling, in one place (spec section 1.1, "Placement table").

Two layouts are recognised; nothing else in the tooling hard-wires a path.

  LAYOUT 'spec_work'  this directory is  $W/spec_work/ref        (the lead modeller's working tree, not published; ROOT = $W)
  LAYOUT 'repo'       this directory is  <repo>/scripts/world     (the stage-1 target; REPO = <repo>)
                      or any tree laid out as 'repo' named by the environment variable WORLD_ROOT.

  logical name            spec_work layout                         repo layout
  ----------------------  ---------------------------------------  --------------------------------------------------------
  P* table                spec_work/pstar_blocs.json               scripts/world/pstar.json
  targets registry        spec_work/targets.json                   scripts/world/targets.json
  bloc CSVs               spec_work/data/                          data/world/research/blocs/data/
  research params.json    research/{blocs,trade,oil,finance}/      data/world/research/{blocs,trade,oil,finance}/
  oil CSVs                research/oil/data/                       data/world/research/oil/data/
  oil prototype           research/oil/scripts/oil_market_model.py scripts/world/oil_reference.py
  golden files            spec_work/golden_*                       crates/econsim-core/src/world/data/golden/<short name>
  generated side files    spec_work/                               crates/econsim-core/src/world/data/golden/
  ABM run files           $WORLD_RUNS (no default)                 $WORLD_RUNS (no default)

`python3 -B paths.py` prints the detected layout and checks that every file exists (exit 1 if one is missing).
`python3 -B make_layout.py DEST` builds a 'repo' tree from the 'spec_work' tree (used as the acceptance check of the placement table).
"""
import os, sys

HERE = os.path.dirname(os.path.abspath(__file__))

# golden logical name -> (file name in the spec_work layout, file name in the repo layout)
GOLDEN_FILES = {
    'stage1':        ('golden_stage1.json',                'stage1.json'),
    'inputs_clean':  ('golden_inputs_in2019_clean.jsonl',  'inputs_in2019_clean.jsonl'),
    'inputs_stress': ('golden_inputs_stress.jsonl',        'inputs_stress.jsonl'),
    'irf_unit':      ('golden_irf_unit.json',              'irf_unit.json'),
    'pf_ar1_EA':     ('golden_pf_ar1_EA.json',             'pf_ar1_EA.json'),
    'kref':          ('golden_kref.json',                  'kref.json'),
    'trade_hand':    ('golden_trade_hand.json',            'trade_hand.json'),
    'oil_activity':  ('oil_activity_results.json',         'oil_activity_results.json'),
    'oil_golden':    ('golden_oil.json',                    'oil_golden.json'),
}


def _detect():
    env = os.environ.get('WORLD_ROOT')
    if env:
        return 'repo', os.path.abspath(env)
    parent = os.path.dirname(HERE)
    if os.path.basename(parent) == 'spec_work':
        return 'spec_work', os.path.dirname(parent)
    return 'repo', os.path.dirname(os.path.dirname(HERE))


LAYOUT, ROOT = _detect()

if LAYOUT == 'spec_work':
    SCRIPTS = HERE
    PSTAR = os.path.join(ROOT, 'spec_work', 'pstar_blocs.json')
    TARGETS = os.path.join(ROOT, 'spec_work', 'targets.json')
    CSV_DIR = os.path.join(ROOT, 'spec_work', 'data')
    RESEARCH_DIR = os.path.join(ROOT, 'research')
    OIL_DATA_DIR = os.path.join(RESEARCH_DIR, 'oil', 'data')
    OIL_PROTOTYPE = os.path.join(RESEARCH_DIR, 'oil', 'scripts', 'oil_market_model.py')
    GOLDEN_DIR = os.path.join(ROOT, 'spec_work')
    _default_runs = None
else:
    SCRIPTS = os.path.join(ROOT, 'scripts', 'world')
    PSTAR = os.path.join(SCRIPTS, 'pstar.json')
    TARGETS = os.path.join(SCRIPTS, 'targets.json')
    RESEARCH_DIR = os.path.join(ROOT, 'data', 'world', 'research')
    CSV_DIR = os.path.join(RESEARCH_DIR, 'blocs', 'data')
    OIL_DATA_DIR = os.path.join(RESEARCH_DIR, 'oil', 'data')
    OIL_PROTOTYPE = os.path.join(SCRIPTS, 'oil_reference.py')
    GOLDEN_DIR = os.path.join(ROOT, 'crates', 'econsim-core', 'src', 'world', 'data', 'golden')
    _default_runs = None

RUNS_DIR = os.environ.get('WORLD_RUNS') or _default_runs
CACHE_DIR = os.path.join(HERE, '.cache')


def research_json(which):
    """Path of the research params.json of 'blocs', 'trade', 'oil' or 'finance'."""
    return os.path.join(RESEARCH_DIR, which, 'params.json')


def golden(name):
    """Path of a golden file by logical name (see GOLDEN_FILES)."""
    return os.path.join(GOLDEN_DIR, GOLDEN_FILES[name][0 if LAYOUT == 'spec_work' else 1])


def csv(name):
    return os.path.join(CSV_DIR, name)


def oil_csv(name):
    return os.path.join(OIL_DATA_DIR, name)


def required():
    """Files that must exist for selftest / gates / kcheck / goldens / trade_hand (golden outputs are not required: they are written)."""
    req = [PSTAR, TARGETS, csv('bloc_spillover_coeffs_2024.csv'), csv('bloc_trade_weights_2024.csv')]
    req += [research_json(w) for w in ('blocs', 'trade', 'oil', 'finance')]
    req += [OIL_PROTOTYPE]
    return req


def check():
    missing = [p for p in required() if not os.path.exists(p)]
    return missing


if __name__ == '__main__':
    print('layout', LAYOUT, 'root', ROOT)
    for k in ('PSTAR', 'TARGETS', 'CSV_DIR', 'RESEARCH_DIR', 'OIL_DATA_DIR', 'OIL_PROTOTYPE', 'GOLDEN_DIR', 'RUNS_DIR'):
        print('  %-14s %s' % (k, globals()[k]))
    miss = check()
    print('missing:', miss if miss else 'none')
    sys.exit(1 if miss else 0)
