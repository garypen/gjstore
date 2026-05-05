use gjstore::gjstore::SharedStore;
use serde_json::Value;
use serde_json::json;
use std::thread;
use std::time::Duration;

fn main() {
    // Initial state with some nested structure
    let initial_data = json!({
        "metadata": {
            "version": 1,
            "status": "initial"
        },
        "counters": {
            "a": 0,
            "b": 0
        }
    });

    let store = SharedStore::builder().value(initial_data).build();

    // Capture the initial generation
    let gen0 = store.latest().expect("Store should have initial value");
    let gen0_val: Value = (*gen0).clone().into();
    println!("Generation 0 captured: {}", gen0_val);

    thread::scope(|s| {
        // Writer thread: Updates the store repeatedly
        let s_writer = &store;
        s.spawn(|| {
            for i in 1..=5 {
                thread::sleep(Duration::from_millis(100));
                println!("Writer: Updating to generation {}", i);
                s_writer
                    .update(json!({
                        "metadata": { "version": i + 1 },
                        "counters": { "a": i * 10 }
                    }))
                    .unwrap();
            }
        });

        // Reader thread 1: Holds onto Gen 0 and compares it later
        let s_reader1 = &store;
        s.spawn(|| {
            println!("Reader 1: I'm holding onto Gen 0...");
            thread::sleep(Duration::from_millis(300));

            let current = s_reader1.latest().unwrap();
            let current_val: Value = (*current).clone().into();

            println!("Reader 1: Still have Gen 0! Value: {}", gen0_val);
            println!("Reader 1: Latest is now: {}", current_val);

            // Prove structural sharing: metadata.status should be the same pointer (conceptually)
            // since we didn't touch it.
        });

        // Reader thread 2: Periodically polls latest
        let s_reader2 = &store;
        s.spawn(|| {
            for _ in 0..6 {
                thread::sleep(Duration::from_millis(80));
                let latest = s_reader2.latest().unwrap();
                let val: Value = (*latest).clone().into();
                println!("Reader 2: Observed latest counters: {}", val["counters"]);
            }
        });
    });

    let final_gen = store.latest().unwrap();
    let final_val: Value = (*final_gen).clone().into();
    println!("Final state: {}", final_val);
}
