use gjstore::gjstore::Store;
use serde_json::json;

fn main() {
    let initial = json!({
        "project": "gjstore",
        "features": ["generational", "sharing"]
    });

    let mut store = Store::builder().value(initial).build();

    // Keep a reference to the oldest generation so that it is preserved
    // past the update. If we didn't take the reference here, then the
    // gc triggered by an update would remove that generation.
    let oldest = store.oldest().unwrap();

    // Apply a merge patch
    store.update(json!({
        "features": ["generational", "sharing", "patching"],
        "version": "0.1.0"
    }));

    // Access generations
    let latest = store.latest().unwrap();
    println!("Latest: {:?}", latest);

    println!("Oldest: {:?}", oldest);
}
