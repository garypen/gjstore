use gjstore::gjstore::SharedStore;
use serde_json::json;
use std::sync::Arc;
use std::thread;

fn main() {
    let store = Arc::new(SharedStore::new(json!({"count": 0})));

    let mut handles = vec![];
    for i in 0..10 {
        let s = Arc::clone(&store);
        handles.push(thread::spawn(move || {
            s.update(json!({"count": i}));
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    println!("Final state: {:?}", store.latest().unwrap());
}
