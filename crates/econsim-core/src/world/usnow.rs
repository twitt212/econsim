//! Today's-start reference of the trade block (docs/CALIBRATION.md, section world-usnow): the US duty by origin and goods group, the origin shares of
//! imports, the destination shares of exports and the partners' retaliatory duties, as measured at a dated vintage. Installed once,
//! at the end of the steered window (`start::build_us_now`), by `world::rebase_usnow` (`TradeState::rebase_today`). Everything the block does afterwards is
//! measured against this reference, so with no change the block holds today's state, and a `Tariff` adds points to today's duty.
//! The block with no reference is the merged code, unchanged.

use serde::Deserialize;

use super::NB;
use super::trade::RET_GOODS;

const REF_JSON: &str = include_str!("data/usnow_ref.json");

/// One dated vintage of the reference. Rates are decimal. `omega[j][s]` is bloc j's share of US imports of goods group s (each
/// column sums to one); `export_dest[j][s]` is bloc j's share of US exports of goods group s (columns sum to one); `ret[j]` is the
/// average duty bloc j puts on US goods (total, goods-weighted).
#[derive(Clone, Debug, Deserialize)]
pub struct RefVintage {
    pub as_of: String,
    pub duty: Vec<Vec<f64>>,
    pub omega: Vec<Vec<f64>>,
    pub export_dest: Vec<Vec<f64>>,
    pub ret: Vec<f64>,
}

#[derive(Deserialize)]
struct RefFile {
    vintages: std::collections::BTreeMap<String, RefVintage>,
    latest: String,
}

fn file() -> &'static RefFile {
    static CELL: std::sync::OnceLock<RefFile> = std::sync::OnceLock::new();
    CELL.get_or_init(|| serde_json::from_str(REF_JSON).expect("usnow_ref.json parses"))
}

/// The id of the newest compiled-in vintage.
pub fn latest_id() -> &'static str {
    &file().latest
}

/// The compiled-in vintage `id`, when it exists and has the right shape.
pub fn vintage(id: &str) -> Option<&'static RefVintage> {
    let v = file().vintages.get(id)?;
    let ok = |m: &Vec<Vec<f64>>| m.len() == NB && m.iter().all(|r| r.len() == RET_GOODS && r.iter().all(|x| x.is_finite() && *x >= 0.0));
    if ok(&v.duty) && ok(&v.omega) && ok(&v.export_dest) && v.ret.len() == NB && v.ret.iter().all(|x| x.is_finite() && *x >= 0.0) {
        Some(v)
    } else {
        None
    }
}

/// Whether `id` names a compiled-in vintage (the request check).
pub fn known(id: &str) -> bool {
    vintage(id).is_some()
}
