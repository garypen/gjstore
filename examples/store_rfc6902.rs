use gjstore::gjstore::Store;
use serde_json::json;

fn main() {
    let initial = json!({
        "project": "gjstore",
        "features": ["generational", "sharing"]
    });

    let mut store = Store::builder().value(initial).build();

    // Keep a reference to the oldest generation so that it is preserved
    let oldest = store.oldest().unwrap();

    // Apply a JSON Patch (RFC 6902)
    // This mirrors the Merge Patch in store.rs:
    // {"features": ["generational", "sharing", "patching"], "version": "0.1.0"}
    store.update(json!([
        {"op": "replace", "path": "/features", "value": ["generational", "sharing", "patching"]},
        {"op": "add", "path": "/version", "value": "0.1.0"}
    ])).expect("Patch should apply");

    // Access generations
    let latest = store.latest().unwrap();
    println!("Latest: {:?}", latest);

    println!("Oldest: {:?}", oldest);
}
