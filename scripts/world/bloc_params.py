"""Single source of truth for the 12 bloc calibrations (quarterly GPM units).  Used by world_net.py and the research scripts that
wrote the blocs params.json (not published).

Quarterly coefficient keys: b1 lag, b2 lead, b3 medium-term real-rate gap, b4 REER gap, b5 foreign-activity, l1 inflation lead weight,
l2 output gap, l3 REER-gap change, g1 smoothing, g2 inflation gap (long-run coefficient = 1+g2), g4 output gap, phi UIP forward weight,
uip_w UIP weight (1 = free float, <1 managed/capital controls), theta global financial channel (IS, per pp of US policy-rate deviation,
lag 2 quarters).
"""
import copy
from bloc_model import GPM6

BLOCS12 = ['EA', 'CN', 'JP', 'UK', 'CA', 'MX', 'IN', 'KR', 'BR', 'AU', 'OPEC+', 'RoW']

# composite weights for RoW coefficients (market-rate GDP shares of RoW groups, from the blocs research): EM Asia + Asian hubs -> EA6 column,
# Latin America ex BR/MX -> LA6 column, Europe/Turkey/Israel/MENA/Africa/other -> RC6 column
ROW_W = {'EA6': 0.2309 + 0.0981, 'LA6': 0.1398, 'RC6': 1 - (0.2309 + 0.0981) - 0.1398}

COL = {'EA': 'EA', 'CN': 'EA6', 'JP': 'JP', 'UK': 'RC6', 'CA': 'RC6', 'MX': 'LA6', 'IN': 'EA6', 'KR': 'EA6', 'BR': 'LA6', 'AU': 'RC6', 'OPEC+': 'RC6'}

OVER = {   # country-specific overrides (quarterly units)
    'CN': dict(b1=.47, b2=.21, b3=.20, b4=.17, l1=.72, l2=.20, l3=.25, g1=.67, g2=1.11, g4=.17, phi=.80, uip_w=.34),     # GPM7 China (Blagrave et al. 2013)
    'IN': dict(b1=.60, b2=.07, b3=.08, b4=.04, l1=.45, l2=.10, l3=.10, g1=.85, g2=2.0, g4=.5, phi=.80, uip_w=.70),       # RBI-IMF QPM (Benes et al. 2017)
}
SCALE_B34 = {'UK': 0.5, 'CA': 0.5, 'AU': 0.7}       # scale on (b3,b4): FSGM / Cloyne-Hurtgen (UK), ToTEM III (CA), MARTIN (AU)
UIPW = {'OPEC+': 0.60}                              # share of OPEC+ GDP with a freely floating or managed-float currency (the rest pegged to USD/EUR)
RHO_G, L_G = 0.80, 1          # persistence of the global financial-conditions state G_t = rho*G_{t-1} + (1-rho)*i_us(t-L)  (quarterly)
THETA = {'EA': .202, 'JP': .202, 'UK': .202, 'CA': .202, 'AU': .202, 'MX': .263, 'BR': .263, 'IN': .263, 'KR': .263, 'RoW': .249, 'CN': .062, 'OPEC+': .124}
LAMBDA_RP = {b: 1.0 for b in ['EA', 'JP', 'UK', 'CA', 'AU', 'MX', 'BR', 'IN', 'KR', 'RoW', 'OPEC+']}; LAMBDA_RP['CN'] = 0.3   # risk-premium loading on G_t (annualised pp per pp of G)
K_US = {'EA': 0.0, 'JP': 0.0, 'UK': 0.0, 'CA': 0.0, 'AU': 0.0, 'MX': 0.4, 'BR': 0.4, 'IN': 0.4, 'KR': 0.4, 'RoW': 0.33, 'CN': 0.2, 'OPEC+': 0.325}
KEYS = ['b1', 'b2', 'b3', 'b4', 'b5', 'l1', 'l2', 'l3', 'g1', 'g2', 'g4', 'phi']


def quarterly(b):
    if b == 'RoW':
        q = {k: sum(ROW_W[c] * GPM6[c][k] for c in ROW_W) for k in KEYS}
        q['uip_w'] = 1 - 0.0704 - 0.0593 * 0.5     # USD pegs fully pegged, EUR pegs half
    else:
        q = {k: GPM6[COL[b]][k] for k in KEYS}
        q.update(OVER.get(b, {}))
        sc = SCALE_B34.get(b, 1.0)
        q['b3'] *= sc; q['b4'] *= sc
        q.setdefault('uip_w', UIPW.get(b, 1.0))
    q['theta'] = THETA[b]
    q['k_us'] = K_US[b]
    return q


def all_quarterly():
    return {b: quarterly(b) for b in BLOCS12}
