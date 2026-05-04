# gjstore

A Generational JSON Store for Rust.

`gjstore` is a library that provides a thread-safe, versioned JSON store with structural sharing. It allows you to maintain multiple generations of a JSON document, applying updates via RFC 7396 Merge Patching while keeping memory usage efficient.

## Features

- Generational Updates: Every update creates a new version (generation) of the store.
- Structural Sharing: Uses `Arc` and Copy-on-Write (COW) patterns to share data between generations.
- RFC 7396 Merge Patch: Supports standard JSON merge patching for updates.
- Automatic Garbage Collection: Automatically removes older generations that are no longer referenced.
- Thread Safety: Provides `SharedStore` for concurrent access with optimized locking.
- Periodic Rebasing: Performs periodic deep copies to ensure memory locality and prevent long-lived objects from pinning memory.

## Usage

### Basic Store

The `Store` type is suitable for single-threaded usage.

```rust
use gjstore::gjstore::Store;
use serde_json::json;

fn main() {
    let initial = json!({
        "project": "gjstore",
        "features": ["generational", "sharing"]
    });

    let mut store = Store::new(initial);

    // Apply a merge patch
    store.update(json!({
        "features": ["generational", "sharing", "patching"],
        "version": "0.1.0"
    }));

    // Access generations
    let latest = store.latest().unwrap();
    println!("Latest: {:?}", latest);

    let oldest = store.oldest().unwrap();
    println!("Oldest: {:?}", oldest);
}
```

### Thread-Safe SharedStore

The `SharedStore` type is designed for multi-threaded environments. It uses a `RwLock` for concurrent reads and a `Mutex` to serialize updates without blocking readers during patch calculation.

```rust
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
```

## How it works

1. SharedValue: JSON values are converted into a `SharedValue` tree where objects and arrays are wrapped in `Arc`.
2. Merge Patch: When a patch is applied, only the modified branches of the tree are cloned. Untouched branches are shared between the new and old generations.
3. Garbage Collection: The store keeps a history of generations. When the oldest generation's `Arc` count drops to 1 (meaning it is only referenced by the store's history), it is eligible for removal.
4. Rebase: Every 20 generations, the store performs a deep clone of the latest value to consolidate memory and break references to old structural fragments.

## License

This project is licensed under the MIT License or Apache 2.0 (check Cargo.toml for details if specified).
