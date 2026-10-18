//! The `fields` filter (docs/API.md, "Field filter"). `fields=a,b,c` on the endpoints that return arrays of `Metrics`
//! keeps only those top-level keys of each monthly record, plus the month key `tick`. `fields=world.imports_bn` keeps
//! one child of the nested object `world` (one level). It runs on the response after the compare cache, so the cache
//! keeps the full bodies and its key is the request body alone. Without a filter nothing here runs and the bodies are
//! exactly as before.

use std::collections::{BTreeMap, HashSet};

use serde_json::value::RawValue;

/// The key that identifies a monthly record. It is kept whether or not `fields` names it.
pub const MONTH_KEY: &str = "tick";

/// The keys named by a `fields` parameter: whole top-level keys, and children of nested objects (`parent.child`).
#[derive(Debug, Clone)]
pub struct FieldSet {
    names: HashSet<String>,
    children: BTreeMap<String, HashSet<String>>,
}

impl FieldSet {
    /// The filter for a raw `fields` value, or `None` (no filter) when the parameter is absent or names nothing: `fields=`,
    /// or only commas and spaces. Names are split on commas and trimmed. A name with one dot (`parent.child`) names a
    /// child of the object at `parent`; a name with more dots matches no child. Unknown names stay in the set and match
    /// nothing.
    pub fn parse(raw: Option<&str>) -> Option<FieldSet> {
        let mut set = FieldSet { names: HashSet::new(), children: BTreeMap::new() };
        for name in raw?.split(',').map(str::trim).filter(|name| !name.is_empty()) {
            match name.split_once('.') {
                Some((parent, child)) => {
                    set.children.entry(parent.trim().to_owned()).or_default().insert(child.trim().to_owned());
                }
                None => {
                    set.names.insert(name.to_owned());
                }
            }
        }
        (!set.names.is_empty() || !set.children.is_empty()).then_some(set)
    }

    /// What a record keeps of one of its keys: the value as it is (the month key, or a whole name, which wins over the
    /// children of the same key), the object narrowed to its listed children, or nothing. A value that is not an object
    /// has no children, so a key named only by children is dropped.
    fn keep_value(&self, key: &str, value: Box<RawValue>) -> serde_json::Result<Option<Box<RawValue>>> {
        if key == MONTH_KEY || self.names.contains(key) {
            return Ok(Some(value));
        }
        let Some(wanted) = self.children.get(key) else { return Ok(None) };
        let Ok(mut object) = serde_json::from_str::<BTreeMap<String, Box<RawValue>>>(value.get()) else {
            return Ok(None);
        };
        object.retain(|child, _| wanted.contains(child));
        if object.is_empty() {
            return Ok(None);
        }
        Ok(Some(RawValue::from_string(serde_json::to_string(&object)?)?))
    }
}

/// Filter the record arrays named in `arrays` of a JSON object (a compare response, or a sim response) and serialise
/// the result. Every other key of the object is unchanged. Values are carried as their original JSON text (`RawValue`),
/// not parsed and printed again, so every kept value is bit-for-bit the value of the unfiltered response. Object keys come
/// out in sorted order, so a filtered body is not byte-identical to an unfiltered one.
pub fn filter_bytes(body: &[u8], arrays: &[&str], keep: &FieldSet) -> serde_json::Result<Vec<u8>> {
    let mut top: BTreeMap<String, Box<RawValue>> = serde_json::from_slice(body)?;
    for name in arrays {
        let Some(raw) = top.get_mut(*name) else { continue };
        let records: Vec<BTreeMap<String, Box<RawValue>>> = serde_json::from_str(raw.get())?;
        let mut kept = Vec::with_capacity(records.len());
        for record in records {
            let mut out = BTreeMap::new();
            for (key, value) in record {
                if let Some(value) = keep.keep_value(&key, value)? {
                    out.insert(key, value);
                }
            }
            kept.push(out);
        }
        *raw = RawValue::from_string(serde_json::to_string(&kept)?)?;
    }
    serde_json::to_vec(&top)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    /// Two records per side, with a nested `world` object and a top-level array of arrays.
    const BODY: &str = r#"{"baseline":[{"tick":0,"date":2026.0,"cpi":1.5,"keep":true,"world":{"y_gap":[1.0,2.0],"z":{"deep":3}}}],"shocked":[{"tick":0,"date":2026.0,"cpi":2.5,"keep":false,"world":{"y_gap":[3.0],"z":{"deep":4}}}],"seeds":[1,2],"warmup_months":24}"#;

    fn filtered(fields: &str) -> Value {
        let keep = FieldSet::parse(Some(fields)).expect("a filter");
        serde_json::from_slice(&filter_bytes(BODY.as_bytes(), &["baseline", "shocked"], &keep).unwrap()).unwrap()
    }

    fn original() -> Value {
        serde_json::from_str(BODY).unwrap()
    }

    #[test]
    fn absent_or_empty_fields_is_no_filter() {
        assert!(FieldSet::parse(None).is_none());
        assert!(FieldSet::parse(Some("")).is_none());
        assert!(FieldSet::parse(Some(" , ,")).is_none());
        assert!(FieldSet::parse(Some("cpi")).is_some());
    }

    #[test]
    fn keeps_only_listed_keys_and_the_month_key() {
        let v = filtered("cpi");
        assert_eq!(v["baseline"][0], json!({"tick": 0, "cpi": 1.5}));
        assert_eq!(v["shocked"][0], json!({"tick": 0, "cpi": 2.5}));
    }

    #[test]
    fn month_key_is_kept_when_not_listed_and_when_listed() {
        assert_eq!(filtered("keep")["baseline"][0], json!({"tick": 0, "keep": true}));
        assert_eq!(filtered("tick")["baseline"][0], json!({"tick": 0}));
    }

    #[test]
    fn unknown_names_are_ignored() {
        assert_eq!(filtered("cpi,nope, ,")["baseline"][0], json!({"tick": 0, "cpi": 1.5}));
        assert_eq!(filtered("nope")["shocked"][0], json!({"tick": 0}));
    }

    #[test]
    fn nested_objects_are_kept_whole() {
        let v = filtered("world");
        let full = original();
        assert_eq!(v["baseline"][0]["world"], full["baseline"][0]["world"]);
        assert_eq!(v["shocked"][0]["world"], full["shocked"][0]["world"]);
        assert_eq!(v["baseline"][0]["world"]["z"]["deep"], json!(3));
    }

    /// Without nested names the output bytes are the serialised records (keys sorted, each kept value as its original
    /// text), exactly as the filter wrote them before nested names existed.
    #[test]
    fn top_level_filter_bytes_are_unchanged() {
        let keep = FieldSet::parse(Some("cpi")).unwrap();
        let out = filter_bytes(BODY.as_bytes(), &["baseline", "shocked"], &keep).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            r#"{"baseline":[{"cpi":1.5,"tick":0}],"seeds":[1,2],"shocked":[{"cpi":2.5,"tick":0}],"warmup_months":24}"#
        );
        let keep = FieldSet::parse(Some("world")).unwrap();
        let out = filter_bytes(BODY.as_bytes(), &["baseline", "shocked"], &keep).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            r#"{"baseline":[{"tick":0,"world":{"y_gap":[1.0,2.0],"z":{"deep":3}}}],"seeds":[1,2],"shocked":[{"tick":0,"world":{"y_gap":[3.0],"z":{"deep":4}}}],"warmup_months":24}"#
        );
    }

    /// A kept float keeps its original text: parsing and printing it again can move the last bit (serde_json without
    /// `float_roundtrip`), so the filter must not do that.
    #[test]
    fn kept_values_keep_their_exact_text() {
        let body = r#"{"baseline":[{"tick":0,"x":22994.604202383754,"y":0.1,"drop":1}],"shocked":[],"seeds":[1]}"#;
        let keep = FieldSet::parse(Some("x,y")).unwrap();
        let out = String::from_utf8(filter_bytes(body.as_bytes(), &["baseline", "shocked"], &keep).unwrap()).unwrap();
        assert!(out.contains(r#""x":22994.604202383754"#), "{out}");
        assert!(out.contains(r#""y":0.1"#), "{out}");
        assert!(!out.contains("drop"), "{out}");
    }

    #[test]
    fn other_keys_of_the_object_are_unchanged() {
        let v = filtered("cpi");
        let full = original();
        assert_eq!(v["seeds"], full["seeds"]);
        assert_eq!(v["warmup_months"], full["warmup_months"]);
        assert_eq!(v["baseline"].as_array().unwrap().len(), 1);
        assert_eq!(v["shocked"].as_array().unwrap().len(), 1);
        assert_eq!(v.as_object().unwrap().len(), full.as_object().unwrap().len());
    }

    /// Records with a `world` object of several children, as the world block has them (bloc arrays of 12 numbers, plus a
    /// scalar and a nested object).
    const WORLD_BODY: &str = r#"{"baseline":[{"tick":0,"cpi":1.5,"world":{"imports_bn":[1.25,2.5],"y_gap":[0.1,-0.2],"ret_tau":[0.0,0.1],"c_t":0.5,"z":{"deep":3}}}],"shocked":[{"tick":1,"cpi":2.5,"world":{"imports_bn":[3.0],"y_gap":[0.30000000000000004],"ret_tau":[0.25],"c_t":-0.5,"z":{"deep":4}}}],"seeds":[1],"warmup_months":24}"#;

    fn world_filtered(fields: &str) -> Value {
        let keep = FieldSet::parse(Some(fields)).expect("a filter");
        serde_json::from_slice(&filter_bytes(WORLD_BODY.as_bytes(), &["baseline", "shocked"], &keep).unwrap()).unwrap()
    }

    /// `parent.child` keeps exactly the listed children of the object and the month key; the other children and the other
    /// keys of the record are dropped.
    #[test]
    fn nested_names_keep_only_the_listed_children_and_the_month_key() {
        let v = world_filtered("world.imports_bn,world.y_gap");
        assert_eq!(v["baseline"][0], json!({"tick": 0, "world": {"imports_bn": [1.25, 2.5], "y_gap": [0.1, -0.2]}}));
        assert_eq!(v["shocked"][0], json!({"tick": 1, "world": {"imports_bn": [3.0], "y_gap": [0.30000000000000004]}}));
        assert_eq!(v["seeds"], json!([1]));
    }

    /// A whole `world` name keeps the object whole, and it wins over children of `world` whatever the order.
    #[test]
    fn a_whole_name_beats_its_nested_children() {
        let whole = world_filtered("world");
        assert_eq!(whole["baseline"][0]["world"].as_object().unwrap().len(), 5);
        assert_eq!(world_filtered("world.imports_bn,world"), whole);
        assert_eq!(world_filtered("world,world.imports_bn"), whole);
    }

    /// Unknown children are ignored. A parent none of whose listed children exist is dropped, as an unknown top-level name
    /// is. So is a parent that is not an object (`cpi` is a number), and an unknown parent.
    #[test]
    fn unknown_children_are_ignored_and_a_parent_with_none_is_dropped() {
        assert_eq!(world_filtered("world.nope,world.y_gap,world.")["baseline"][0], json!({"tick": 0, "world": {"y_gap": [0.1, -0.2]}}));
        let v = world_filtered("world.nope");
        assert_eq!(v["baseline"][0], json!({"tick": 0}));
        assert_eq!(v["shocked"][0], json!({"tick": 1}));
        assert_eq!(world_filtered("nope.y_gap,cpi.x,world.a.b")["baseline"][0], json!({"tick": 0}));
    }

    /// The kept children are the same JSON text as in the unfiltered body (no re-printed floats), and nothing else of the
    /// object is in the output.
    #[test]
    fn kept_children_keep_their_exact_text() {
        let body = r#"{"baseline":[{"tick":0,"world":{"x":22994.604202383754,"y":0.1,"drop":[1.0000000000000002]}}],"shocked":[],"seeds":[1]}"#;
        let keep = FieldSet::parse(Some("world.x,world.y")).unwrap();
        let out = String::from_utf8(filter_bytes(body.as_bytes(), &["baseline", "shocked"], &keep).unwrap()).unwrap();
        assert_eq!(out, r#"{"baseline":[{"tick":0,"world":{"x":22994.604202383754,"y":0.1}}],"seeds":[1],"shocked":[]}"#);
    }
}
