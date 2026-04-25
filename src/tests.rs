use crate::gjstore::*;
use serde_json::{json, Value};
use std::sync::Arc;

fn shared_to_json(sv: &SharedValue) -> Value {
    match sv {
        SharedValue::Leaf(v) => v.clone(),
        SharedValue::Array(a) => Value::Array(a.iter().map(shared_to_json).collect()),
        SharedValue::Object(o) => {
            Value::Object(o.iter().map(|(k, v)| (k.clone(), shared_to_json(v))).collect())
        }
    }
}

#[test]
fn test_initialization() {
    let initial = json!({"a": 1, "b": [1, 2, 3]});
    let store = SharedStore::new(initial.clone());
    
    let gen0 = store.get(0).expect("Generation 0 should exist");
    assert_eq!(shared_to_json(&gen0), initial);
}

#[test]
fn test_basic_update() {
    let store = SharedStore::new(json!({"a": 1, "b": 2}));
    store.update(json!({"a": 3}));
    
    let gen1 = store.get(1).expect("Generation 1 should exist");
    assert_eq!(shared_to_json(&gen1), json!({"a": 3, "b": 2}));
}

#[test]
fn test_null_patch_removes_key() {
    let store = SharedStore::new(json!({"a": 1, "b": 2}));
    store.update(json!({"a": null}));
    
    let gen1 = store.get(1).expect("Generation 1 should exist");
    assert_eq!(shared_to_json(&gen1), json!({"b": 2}));
}

#[test]
fn test_nested_update() {
    let store = SharedStore::new(json!({"a": {"x": 1, "y": 2}}));
    store.update(json!({"a": {"x": 3}}));
    
    let gen1 = store.get(1).expect("Generation 1 should exist");
    assert_eq!(shared_to_json(&gen1), json!({"a": {"x": 3, "y": 2}}));
}

#[test]
fn test_structural_sharing() {
    let initial = json!({"a": {"x": 1}, "b": {"y": 2}});
    let store = SharedStore::new(initial);
    
    let gen0 = store.get(0).unwrap();
    store.update(json!({"a": {"x": 3}}));
    let gen1 = store.get(1).unwrap();
    
    if let (SharedValue::Object(m0), SharedValue::Object(m1)) = (&*gen0, &*gen1) {
        // "b" should be shared
        let b0 = m0.get("b").unwrap();
        let b1 = m1.get("b").unwrap();
        
        if let (SharedValue::Object(arc0), SharedValue::Object(arc1)) = (b0, b1) {
            assert!(Arc::ptr_eq(arc0, arc1), "Unchanged branch 'b' should share the same Arc");
        } else {
            panic!("Expected objects for 'b'");
        }
        
        // "a" should NOT be shared (it was patched)
        let a0 = m0.get("a").unwrap();
        let a1 = m1.get("a").unwrap();
        if let (SharedValue::Object(arc0), SharedValue::Object(arc1)) = (a0, a1) {
            assert!(!Arc::ptr_eq(arc0, arc1), "Changed branch 'a' should NOT share the same Arc");
        } else {
            panic!("Expected objects for 'a'");
        }
    } else {
        panic!("Expected objects");
    }
}

#[test]
fn test_gc_behavior() {
    let store = SharedStore::new(json!({"count": 0}));
    
    store.update(json!({"count": 1})); // gen 0 should be dropped
    assert!(store.get(0).is_none(), "Generation 0 should have been GC'd");
    assert!(store.get(1).is_some());
    
    // Now hold onto gen 1
    let gen1 = store.get(1).unwrap();
    
    store.update(json!({"count": 2})); // gen 1 kept because we hold it
    assert!(store.get(1).is_some(), "Generation 1 should be kept because we hold an Arc");
    
    store.update(json!({"count": 3})); // gen 3 created, gen 1 still kept
    assert!(store.get(1).is_some());
    assert!(store.get(2).is_some(), "Generation 2 should be kept because gen 1 is blocking the front of the queue");
    
    drop(gen1);
    store.update(json!({"count": 4})); // gen 4 created, gen 1, 2, 3 should be dropped
    assert!(store.get(1).is_none(), "Generation 1 should have been GC'd");
    assert!(store.get(2).is_none(), "Generation 2 should have been GC'd");
    assert!(store.get(3).is_none(), "Generation 3 should have been GC'd");
    assert!(store.get(4).is_some());
}

#[test]
fn test_deep_clone_rebase() {
    let store = SharedStore::new(json!({"obj": {"x": 1}, "other": 0}));
    let _gen0 = store.get(0).unwrap(); // Hold gen 0 to keep history
    
    for i in 1..20 {
        store.update(json!({"other": i}));
    }
    
    let gen19 = store.get(19).expect("Generation 19 should exist after 19 updates");
    
    // At gen 19, "obj" should still be sharing the same Arc from gen 0.
    // Trigger rebase at next_gen = 20
    store.update(json!({"other": 20}));
    let gen20 = store.get(20).expect("Generation 20 should exist");
    
    if let (SharedValue::Object(m19), SharedValue::Object(m20)) = (&*gen19, &*gen20) {
        let obj19 = m19.get("obj").unwrap();
        let obj20 = m20.get("obj").unwrap();
        
        if let (SharedValue::Object(arc19), SharedValue::Object(arc20)) = (obj19, obj20) {
            assert!(!Arc::ptr_eq(arc19, arc20), "Rebase should have created a new Arc even for unchanged branches");
        }
    }
}

#[test]
fn test_patch_array_replaces() {
    let store = SharedStore::new(json!({"a": [1, 2]}));
    store.update(json!({"a": [3, 4]})); // RFC 7396: arrays are replaced, not merged
    
    let gen1 = store.get(1).unwrap();
    assert_eq!(shared_to_json(&gen1), json!({"a": [3, 4]}));
}
