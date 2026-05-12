use crate::gjstore::*;
use serde_json::{Value, json};
use std::sync::Arc;

fn shared_to_json(sv: &SharedValue) -> Value {
    match sv {
        SharedValue::Leaf(v) => v.clone(),
        SharedValue::Array(a) => Value::Array(a.iter().map(shared_to_json).collect()),
        SharedValue::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), shared_to_json(v)))
                .collect(),
        ),
    }
}

#[test]
fn test_initialization() {
    let initial = json!({"a": 1, "b": [1, 2, 3]});
    let store = SharedStore::builder().value(initial.clone()).build();

    let gen0 = store.get(0).expect("Generation 0 should exist");
    assert_eq!(shared_to_json(&gen0), initial);
}

#[test]
fn test_initial_label() {
    let initial = json!({"a": 1});
    let store = SharedStore::builder()
        .value(initial)
        .label("initial".to_string())
        .build();

    let gen0 = store.get_by_label("initial").expect("Label 'initial' should exist");
    assert_eq!(shared_to_json(&gen0), json!({"a": 1}));
    assert_eq!(store.get_generation_by_label("initial"), Some(0));
}

#[test]
fn test_rfc7396_basic_update() {
    let store = SharedStore::builder()
        .value(json!({"a": 1, "b": 2}))
        .build();
    store.update(json!({"a": 3})).unwrap();

    let gen1 = store.get(1).expect("Generation 1 should exist");
    assert_eq!(shared_to_json(&gen1), json!({"a": 3, "b": 2}));
}

#[test]
fn test_rfc7396_null_patch_removes_key() {
    let store = SharedStore::builder()
        .value(json!({"a": 1, "b": 2}))
        .build();
    store.update(json!({"a": null})).unwrap();

    let gen1 = store.get(1).expect("Generation 1 should exist");
    assert_eq!(shared_to_json(&gen1), json!({"b": 2}));
}

#[test]
fn test_rfc7396_nested_update() {
    let store = SharedStore::builder()
        .value(json!({"a": {"x": 1, "y": 2}}))
        .build();
    store.update(json!({"a": {"x": 3}})).unwrap();

    let gen1 = store.get(1).expect("Generation 1 should exist");
    assert_eq!(shared_to_json(&gen1), json!({"a": {"x": 3, "y": 2}}));
}

#[test]
fn test_rfc7396_structural_sharing() {
    let initial = json!({"a": {"x": 1}, "b": {"y": 2}});
    let store = SharedStore::builder().value(initial).build();

    let gen0 = store.get(0).unwrap();
    store.update(json!({"a": {"x": 3}})).unwrap();
    let gen1 = store.get(1).unwrap();

    if let (SharedValue::Object(m0), SharedValue::Object(m1)) = (&*gen0, &*gen1) {
        // "b" should be shared
        let b0 = m0.get("b").unwrap();
        let b1 = m1.get("b").unwrap();

        if let (SharedValue::Object(arc0), SharedValue::Object(arc1)) = (b0, b1) {
            assert!(
                Arc::ptr_eq(arc0, arc1),
                "Unchanged branch 'b' should share the same Arc"
            );
        } else {
            panic!("Expected objects for 'b'");
        }

        // "a" should NOT be shared (it was patched)
        let a0 = m0.get("a").unwrap();
        let a1 = m1.get("a").unwrap();
        if let (SharedValue::Object(arc0), SharedValue::Object(arc1)) = (a0, a1) {
            assert!(
                !Arc::ptr_eq(arc0, arc1),
                "Changed branch 'a' should NOT share the same Arc"
            );
        } else {
            panic!("Expected objects for 'a'");
        }
    } else {
        panic!("Expected objects");
    }
}

#[test]
fn test_gc_behavior() {
    let store = SharedStore::builder().value(json!({"count": 0})).build();

    store.update(json!({"count": 1})).unwrap(); // gen 0 should be dropped
    assert!(store.get(0).is_none(), "Generation 0 should have been GC'd");
    assert!(store.get(1).is_some());

    // Now hold onto gen 1
    let gen1 = store.get(1).unwrap();

    store.update(json!({"count": 2})).unwrap(); // gen 1 kept because we hold it
    assert!(
        store.get(1).is_some(),
        "Generation 1 should be kept because we hold an Arc"
    );

    store.update(json!({"count": 3})).unwrap(); // gen 3 created, gen 1 still kept
    assert!(store.get(1).is_some());
    assert!(
        store.get(2).is_some(),
        "Generation 2 should be kept because gen 1 is blocking the front of the queue"
    );

    drop(gen1);
    store.update(json!({"count": 4})).unwrap(); // gen 4 created, gen 1, 2, 3 should be dropped
    assert!(store.get(1).is_none(), "Generation 1 should have been GC'd");
    assert!(store.get(2).is_none(), "Generation 2 should have been GC'd");
    assert!(store.get(3).is_none(), "Generation 3 should have been GC'd");
    assert!(store.get(4).is_some());
}

#[test]
fn test_deep_clone_rebase() {
    let store = SharedStore::builder()
        .value(json!({"obj": {"x": 1}, "other": 0}))
        .build();
    let _gen0 = store.get(0).unwrap(); // Hold gen 0 to keep history

    for i in 1..20 {
        store.update(json!({"other": i})).unwrap();
    }

    let gen19 = store
        .get(19)
        .expect("Generation 19 should exist after 19 updates");

    // At gen 19, "obj" should still be sharing the same Arc from gen 0.
    // Trigger rebase at next_gen = 20
    store.update(json!({"other": 20})).unwrap();
    let gen20 = store.get(20).expect("Generation 20 should exist");

    if let (SharedValue::Object(m19), SharedValue::Object(m20)) = (&*gen19, &*gen20) {
        let obj19 = m19.get("obj").unwrap();
        let obj20 = m20.get("obj").unwrap();

        if let (SharedValue::Object(arc19), SharedValue::Object(arc20)) = (obj19, obj20) {
            assert!(
                !Arc::ptr_eq(arc19, arc20),
                "Rebase should have created a new Arc even for unchanged branches"
            );
        }
    }
}

#[test]
fn test_rfc7396_patch_array_replaces() {
    let store = SharedStore::builder().value(json!({"a": [1, 2]})).build();
    store.update(json!({"a": [3, 4]})).unwrap(); // RFC 7396: arrays are replaced, not merged

    let gen1 = store.get(1).unwrap();
    assert_eq!(shared_to_json(&gen1), json!({"a": [3, 4]}));
}

#[test]
fn test_rfc6902_basic_update() {
    let store = SharedStore::builder()
        .value(json!({"a": 1, "b": 2}))
        .build();
    // Re-expressing {"a": 3} as JSON Patch
    store
        .update(json!([{"op": "replace", "path": "/a", "value": 3}]))
        .unwrap();

    assert_eq!(
        shared_to_json(&store.latest().unwrap()),
        json!({"a": 3, "b": 2})
    );
}

#[test]
fn test_rfc6902_null_patch_removes_key() {
    let store = SharedStore::builder()
        .value(json!({"a": 1, "b": 2}))
        .build();
    // Re-expressing {"a": null} as JSON Patch
    store
        .update(json!([{"op": "remove", "path": "/a"}]))
        .unwrap();

    assert_eq!(shared_to_json(&store.latest().unwrap()), json!({"b": 2}));
}

#[test]
fn test_rfc6902_nested_update() {
    let store = SharedStore::builder()
        .value(json!({"a": {"x": 1, "y": 2}}))
        .build();
    // Re-expressing {"a": {"x": 3}} as JSON Patch
    store
        .update(json!([{"op": "replace", "path": "/a/x", "value": 3}]))
        .unwrap();

    assert_eq!(
        shared_to_json(&store.latest().unwrap()),
        json!({"a": {"x": 3, "y": 2}})
    );
}

#[test]
fn test_rfc6902_store_example() {
    let initial = json!({
        "project": "gjstore",
        "features": ["generational", "sharing"]
    });
    let store = Store::builder().value(initial).build();
    let mut store = store;

    // Re-expressing the example's merge patch as a JSON Patch
    // Merge Patch: {"features": ["generational", "sharing", "patching"], "version": "0.1.0"}
    store.update(json!([
        {"op": "replace", "path": "/features", "value": ["generational", "sharing", "patching"]},
        {"op": "add", "path": "/version", "value": "0.1.0"}
    ])).unwrap();

    let expected = json!({
        "project": "gjstore",
        "features": ["generational", "sharing", "patching"],
        "version": "0.1.0"
    });
    assert_eq!(shared_to_json(&store.latest().unwrap()), expected);
}

#[test]
fn test_rfc6902_shared_store_example() {
    let initial = json!({
        "metadata": {
            "version": 1,
            "status": "initial"
        },
        "counters": {
            "a": 0,
            "b": 0
        }
    });
    let store = SharedStore::builder().value(initial).build();

    // Re-expressing one iteration of the example loop
    // Merge Patch: {"metadata": {"version": 2}, "counters": {"a": 10}}
    store
        .update(json!([
            {"op": "replace", "path": "/metadata/version", "value": 2},
            {"op": "replace", "path": "/counters/a", "value": 10}
        ]))
        .unwrap();

    let expected = json!({
        "metadata": {
            "version": 2,
            "status": "initial"
        },
        "counters": {
            "a": 10,
            "b": 0
        }
    });
    assert_eq!(shared_to_json(&store.latest().unwrap()), expected);
}

#[test]
fn test_rfc6902_basic_operations() {
    let store = SharedStore::builder()
        .value(json!({"a": 1, "b": [1, 2]}))
        .build();

    // Add to object
    store
        .update(json!([{"op": "add", "path": "/c", "value": 3}]))
        .unwrap();
    assert_eq!(
        shared_to_json(&store.latest().unwrap()),
        json!({"a": 1, "b": [1, 2], "c": 3})
    );

    // Remove from object
    store
        .update(json!([{"op": "remove", "path": "/a"}]))
        .unwrap();
    assert_eq!(
        shared_to_json(&store.latest().unwrap()),
        json!({"b": [1, 2], "c": 3})
    );

    // Replace in object
    store
        .update(json!([{"op": "replace", "path": "/c", "value": 4}]))
        .unwrap();
    assert_eq!(
        shared_to_json(&store.latest().unwrap()),
        json!({"b": [1, 2], "c": 4})
    );

    // Add to array (insert)
    store
        .update(json!([{"op": "add", "path": "/b/1", "value": 1.5}]))
        .unwrap();
    assert_eq!(
        shared_to_json(&store.latest().unwrap()),
        json!({"b": [1, 1.5, 2], "c": 4})
    );

    // Add to array (append)
    store
        .update(json!([{"op": "add", "path": "/b/-", "value": 3}]))
        .unwrap();
    assert_eq!(
        shared_to_json(&store.latest().unwrap()),
        json!({"b": [1, 1.5, 2, 3], "c": 4})
    );
}

#[test]
fn test_rfc6902_move_copy_test() {
    let store = SharedStore::builder()
        .value(json!({"a": {"x": 1}, "b": 2}))
        .build();

    // Copy
    store
        .update(json!([{"op": "copy", "from": "/a", "path": "/c"}]))
        .unwrap();
    assert_eq!(
        shared_to_json(&store.latest().unwrap()),
        json!({"a": {"x": 1}, "b": 2, "c": {"x": 1}})
    );

    // Verify structural sharing after copy
    let latest = store.latest().unwrap();
    if let SharedValue::Object(m) = &*latest {
        let a = m.get("a").unwrap();
        let c = m.get("c").unwrap();
        if let (SharedValue::Object(arc_a), SharedValue::Object(arc_c)) = (a, c) {
            assert!(
                Arc::ptr_eq(arc_a, arc_c),
                "Copied subtree should share the same Arc"
            );
        }
    }

    // Move
    store
        .update(json!([{"op": "move", "from": "/c", "path": "/d"}]))
        .unwrap();
    assert_eq!(
        shared_to_json(&store.latest().unwrap()),
        json!({"a": {"x": 1}, "b": 2, "d": {"x": 1}})
    );

    // Test
    store
        .update(json!([{"op": "test", "path": "/b", "value": 2}]))
        .unwrap();

    // Test failure should fail the whole update
    let res = store.update(json!([
        {"op": "replace", "path": "/b", "value": 3},
        {"op": "test", "path": "/a/x", "value": 99}
    ]));
    assert!(res.is_err());
    assert_eq!(
        shared_to_json(&store.latest().unwrap()),
        json!({"a": {"x": 1}, "b": 2, "d": {"x": 1}})
    );
}

#[test]
fn test_intermingled_rfc7396_rfc6902_patches() {
    let store = SharedStore::builder().value(json!({"a": 1})).build();

    // Merge patch (RFC 7396)
    store.update(json!({"b": 2})).unwrap();

    // JSON patch (RFC 6902)
    store
        .update(json!([{"op": "add", "path": "/c", "value": 3}]))
        .unwrap();

    // Merge patch (RFC 7396)
    store.update(json!({"a": null, "d": 4})).unwrap();

    assert_eq!(
        shared_to_json(&store.latest().unwrap()),
        json!({"b": 2, "c": 3, "d": 4})
    );
}

#[test]
fn test_labels_basic() {
    let store = SharedStore::builder().value(json!({"count": 0})).build();

    // Update with label
    store
        .update_with_label(json!({"count": 1}), "v1".to_string())
        .unwrap();

    let v1 = store.get_by_label("v1").expect("Label v1 should exist");
    assert_eq!(shared_to_json(&v1), json!({"count": 1}));
    assert_eq!(store.get_generation_by_label("v1"), Some(1));

    // Update without label
    store.update(json!({"count": 2})).unwrap();
    assert!(store.get_by_label("v1").is_some());
    assert_eq!(shared_to_json(&store.get_by_label("v1").unwrap()), json!({"count": 1}));
}

#[test]
fn test_label_reassociation() {
    let store = SharedStore::builder().value(json!({"count": 0})).build();

    store.update_with_label(json!({"count": 1}), "tag".to_string()).unwrap();
    assert_eq!(store.get_generation_by_label("tag"), Some(1));

    store.update_with_label(json!({"count": 2}), "tag".to_string()).unwrap();
    assert_eq!(store.get_generation_by_label("tag"), Some(2));
}

#[test]
fn test_labels_and_gc() {
    let store = SharedStore::builder().value(json!({"count": 0})).build();

    // Gen 1 with label
    store.update_with_label(json!({"count": 1}), "keep_me".to_string()).unwrap();

    // Gen 2
    store.update(json!({"count": 2})).unwrap();

    // GC check:
    // history: [gen0, gen1, gen2]
    // gen0 count 1 -> popped
    // gen1 count 1 (weak label) -> popped
    // history: [gen2]

    assert!(store.get(1).is_none(), "Generation 1 should be GC'd even with a label");
    assert!(store.get_by_label("keep_me").is_none(), "Label should be removed when generation is GC'd");

    // Now try with pinning
    store.update_with_label(json!({"count": 3}), "pinned".to_string()).unwrap();
    let _pin = store.get_by_label("pinned").unwrap();

    store.update(json!({"count": 4})).unwrap();
    assert!(store.get_by_label("pinned").is_some(), "Pinned generation should NOT be GC'd");
}
