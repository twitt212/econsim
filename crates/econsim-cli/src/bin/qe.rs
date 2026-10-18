//! QE diagnostic: CB holdings, private duration and the 10y term premium, baseline vs QE.
use econsim_core::{sim, Config, ScheduledShock, Shock};
fn main() {
    let w0 = sim::new_world(&Config { n_households: 5000, ..Default::default() }, 1);
    let mut b = w0.clone();
    let mut s = w0;
    s.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::QE { tenor_idx: 3, monthly_face_pct_gdp: 0.5, months: 12 } });
    for t in 0..18 {
        sim::step(&mut b);
        sim::step(&mut s);
        if t % 3 == 2 {
            let pd = |w: &econsim_core::World| econsim_core::monetary::private_duration(w);
            println!("t{t}: cb_face base {:.3e} qe {:.3e} | priv dur base {:.3e} qe {:.3e} | tp10 base {:.4} qe {:.4} | d0 base {:?} qe {:?} | qe_flow {:.3e}",
                b.bonds.iter().map(|x| x.held_by(econsim_core::types::BondHolder::CentralBank)).sum::<f64>(),
                s.bonds.iter().map(|x| x.held_by(econsim_core::types::BondHolder::CentralBank)).sum::<f64>(),
                pd(&b), pd(&s), b.curve.term_premium[3], s.curve.term_premium[3], b.monetary.d0, s.monetary.d0, s.flows.qe_purchases);
        }
    }
}
