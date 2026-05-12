use gjstore::gjstore::SharedStore;
use serde_json::json;

fn main() {
    let initial = json!({
        "project": "gjstore",
        "status": "initial"
    });

    let store = SharedStore::builder().value(initial).build();

    // Apply an update with a label
    store
        .update_with_label(json!({"status": "v1.0-released"}), "v1.0".to_string())
        .unwrap();

    // Retrieve by label
    let v1 = store.get_by_label("v1.0").unwrap();
    let gen_v1 = store.get_generation_by_label("v1.0").unwrap();

    println!("Generation {} (v1.0): {:?}", gen_v1, v1);

    // Apply more updates
    store.update(json!({"status": "developing-v2"})).unwrap();

    // The label "v1.0" still points to the same generation
    let current_v1 = store.get_by_label("v1.0").unwrap();
    println!("Still have v1.0: {:?}", current_v1);

    // But it doesn't prevent GC if nobody holds a strong reference!
    // (In this example, 'v1' and 'current_v1' are strong references, so it stays)
    drop(v1);
    drop(current_v1);

    // Now it might be GC'd on next update if nothing else holds it
    store.update(json!({"status": "v2-alpha"})).unwrap();

    if store.get_by_label("v1.0").is_none() {
        println!("v1.0 was GC'd as expected because it wasn't pinned.");
    }
}
