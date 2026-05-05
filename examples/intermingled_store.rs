use gjstore::gjstore::SharedStore;
use serde_json::Value;
use serde_json::json;
use std::thread;
use std::time::Duration;

fn main() {
    // Initial state
    let initial_data = json!({
        "app": "intermingle-demo",
        "state": {
            "users": ["alice", "bob"],
            "config": {
                "theme": "light",
                "notifications": true
            }
        }
    });

    let store = SharedStore::builder().value(initial_data).build();

    println!(
        "Initial state: {}",
        serde_json::to_string(&*store.latest().unwrap()).unwrap()
    );

    thread::scope(|s| {
        let s_ref = &store;

        // Writer thread mixing patch types
        s.spawn(|| {
            // 1. Use RFC 7396 (Merge Patch) for a simple top-level addition
            println!("Writer: Adding version via RFC 7396 (Merge Patch)");
            s_ref
                .update(json!({
                    "version": "1.0.0"
                }))
                .unwrap();
            thread::sleep(Duration::from_millis(100));

            // 2. Use RFC 6902 (JSON Patch) for precise array manipulation
            //    Add "charlie" to the users list without replacing the whole array
            println!("Writer: Appending user via RFC 6902 (JSON Patch)");
            s_ref
                .update(json!([
                    {"op": "add", "path": "/state/users/-", "value": "charlie"}
                ]))
                .unwrap();
            thread::sleep(Duration::from_millis(100));

            // 3. Use RFC 7396 (Merge Patch) to update a nested object
            println!("Writer: Updating theme via RFC 7396 (Merge Patch)");
            s_ref
                .update(json!({
                    "state": {
                        "config": { "theme": "dark" }
                    }
                }))
                .unwrap();
            thread::sleep(Duration::from_millis(100));

            // 4. Use RFC 6902 (JSON Patch) for a "test-then-set" operation
            //    Only turn off notifications if the theme is currently "dark"
            println!("Writer: Conditional update via RFC 6902 (JSON Patch)");
            s_ref
                .update(json!([
                    {"op": "test", "path": "/state/config/theme", "value": "dark"},
                    {"op": "replace", "path": "/state/config/notifications", "value": false}
                ]))
                .unwrap();
        });

        // Reader thread observing the transformations
        s.spawn(|| {
            for i in 0..5 {
                thread::sleep(Duration::from_millis(70));
                let latest = s_ref.latest().unwrap();
                let val: Value = (*latest).clone().into();
                println!(
                    "Reader: Observation {}: users count = {}, theme = {}",
                    i,
                    val["state"]["users"]
                        .as_array()
                        .map(|a| a.len())
                        .unwrap_or(0),
                    val["state"]["config"]["theme"]
                );
            }
        });
    });

    let final_val: Value = (*store.latest().unwrap()).clone().into();
    println!(
        "\nFinal State: {}",
        serde_json::to_string_pretty(&final_val).unwrap()
    );
}
