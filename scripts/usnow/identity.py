#!/usr/bin/env python3
"""identity.py BASE_SERVER NEW_SERVER: compare /api/compare bodies byte for byte on two free ports (IDENT_PORT_BASE and IDENT_PORT_NEW, default 8810 and 8811; cache off)."""
import sys, os, json, time, subprocess, urllib.request, hashlib
WT = os.environ.get("ECONSIM_REPO", os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..")))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import uprobe
base_bin, new_bin = sys.argv[1], sys.argv[2]
env = dict(os.environ, RAYON_NUM_THREADS="1", ECONSIM_CACHE_MB="0", ECONSIM_WARMUP_MONTHS="24")
procs = [subprocess.Popen([b], env=dict(env, PORT=str(p)), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) for b, p in ((base_bin, int(os.environ.get("IDENT_PORT_BASE", 8810))), (new_bin, int(os.environ.get("IDENT_PORT_NEW", 8811))))]
time.sleep(2.0)
def post(port, body, path="/api/compare"):
    req = urllib.request.Request(f"http://127.0.0.1:{port}{path}", data=json.dumps(body).encode(), headers={"Content-Type": "application/json"})
    try:
        r = urllib.request.urlopen(req, timeout=3600); return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()
def tar(p, s, pct, r): return {"at_month": 0, "shock": {"type": "Tariff", "partner": p, "sector": s, "pct": pct, "retaliate": r}}
C3 = uprobe.C3; T2A = uprobe.T2A; TG = uprobe.TARGETS
def cfg(**kw):
    c = dict(C3, n_households=1000); c.update(kw); return c
cases = {
 "typical_plain": (cfg(warmup_months=24), [{"at_month": 0, "shock": {"type": "PolicyShift", "bps": 100, "months": 12}}]),
 "typical_importtariff": (cfg(warmup_months=24), [{"at_month": 0, "shock": {"type": "ImportTariff", "pct": 10}}]),
 "typical_world_noshock": (cfg(**T2A, warmup_months=24), []),
 "typical_world_CNr": (cfg(**T2A, warmup_months=24), [tar("CN", None, 25, True)]),
 "typical_world_CAr": (cfg(**T2A, warmup_months=24), [tar("CA", None, 25, True)]),
 "typical_world_U10": (cfg(**T2A, warmup_months=24), [tar(None, None, 10, False)]),
 "typical_world_UK25_noret": (cfg(**T2A, warmup_months=24), [tar("UK", None, 25, False)]),
 "typical_world_UK25_ret": (cfg(**T2A, warmup_months=24), [tar("UK", None, 25, True)]),
 "typical_world_U25_ret": (cfg(**T2A, warmup_months=24), [tar(None, None, 25, True)]),
 "typical_world_ret_off_U25r": (cfg(**dict(T2A, world_retaliation=False), warmup_months=24), [tar(None, None, 25, True)]),
 "usnow_plain": (cfg(start_state="us_now", start_targets=TG), [{"at_month": 0, "shock": {"type": "PolicyShift", "bps": 100, "months": 12}}]),
 "usnow_importtariff": (cfg(start_state="us_now", start_targets=TG), [{"at_month": 0, "shock": {"type": "ImportTariff", "pct": 18.22}}]),
 "usnow_world_none_noshock": (cfg(**T2A, start_state="us_now", start_targets=dict(TG, trade_ref="none")), []),
 "usnow_world_none_CNr": (cfg(**T2A, start_state="us_now", start_targets=dict(TG, trade_ref="none")), [tar("CN", None, 25, True)]),
}
res = {}
for name, (c, shocks) in cases.items():
    body = {"config": c, "shocks": shocks, "months": 12, "seeds": [1, 2]}
    sa, a = post(int(os.environ.get("IDENT_PORT_BASE", 8810)), body); sb, b = post(int(os.environ.get("IDENT_PORT_NEW", 8811)), body)
    same = (sa == sb and a == b)
    res[name] = (sa, sb, same, hashlib.md5(a).hexdigest()[:8], hashlib.md5(b).hexdigest()[:8], len(a))
    print(f"{name:32s} base {sa} new {sb} {'IDENTICAL' if same else 'DIFFERENT'} {res[name][3]} {res[name][4]} {len(a)}B", flush=True)
# config default: key by key
import urllib.request as u
ca = json.loads(u.urlopen("http://127.0.0.1:%d/api/config/default" % int(os.environ.get("IDENT_PORT_BASE", 8810))).read()); cb = json.loads(u.urlopen("http://127.0.0.1:%d/api/config/default" % int(os.environ.get("IDENT_PORT_NEW", 8811))).read())
print("config/default keys only in new:", sorted(set(cb) - set(ca)), "only in base:", sorted(set(ca) - set(cb)), "shared differing:", [k for k in ca if k in cb and ca[k] != cb[k]])
print("start_states new:", json.dumps(cb.get("start_states")))
for p in procs: p.terminate(); p.wait(timeout=30)
