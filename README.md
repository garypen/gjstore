# gjstore

A Generational JSON Store for Rust.

`gjstore` is a library that provides a thread-safe, versioned JSON store with structural sharing. It allows you to maintain multiple generations of a JSON document, applying updates via standard JSON patch formats while keeping memory usage efficient.

## Features

- **Generational Updates**: Every update creates a new version (generation) of the store.
- **Structural Sharing**: Uses `Arc` and Copy-on-Write (COW) patterns to share data between generations.
- **Dual Patch Support**: Automatically detects and applies updates in two standard formats:
  - **RFC 7396 (JSON Merge Patch)**: Best for simple object property updates.
  - **RFC 6902 (JSON Patch)**: Best for precise array manipulation and atomic operations.
- **Efficient Subtree Ops**: `move` and `copy` operations in JSON Patch are $O(1)$ due to structural sharing.
- **Automatic Garbage Collection**: Automatically removes older generations that are no longer referenced.
- **Thread Safety**: Provides `SharedStore` for concurrent access with optimized locking.
- **Periodic Rebasing**: Performs periodic deep copies to ensure memory locality and prevent long-lived objects from pinning memory.

## Usage

### Basic Store

The `Store` type is suitable for single-threaded usage.

```rust
use gjstore::gjstore::Store;
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let initial = json!({
        "project": "gjstore",
        "features": ["generational", "sharing"]
    });

    let mut store = Store::builder()
        .value(initial)
        .interval(20)
        .build();

    // 1. Apply a Merge Patch (RFC 7396) - passing an Object
    store.update(json!({
        "version": "0.1.0"
    }))?;

    // 2. Apply a JSON Patch (RFC 6902) - passing an Array
    store.update(json!([
        {"op": "add", "path": "/features/-", "value": "patching"}
    ]))?;

    let latest = store.latest().unwrap();
    println!("Latest: {:?}", latest);
    Ok(())
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
    let store = Arc::new(SharedStore::builder()
        .value(json!({"count": 0}))
        .build());

    let mut handles = vec![];
    for i in 0..10 {
        let s = Arc::clone(&store);
        handles.push(thread::spawn(move || {
            // update returns a Result, though we unwrap here for simplicity
            s.update(json!({"count": i})).unwrap();
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    println!("Final state: {:?}", store.latest().unwrap());
}
```

## How it works

1. **SharedValue**: JSON values are converted into a `SharedValue` tree where objects and arrays are wrapped in `Arc`.
2. **Structural Sharing**: When a patch is applied, only the modified branches of the tree are cloned (`Arc::make_mut`). Untouched branches are shared between generations.
3. **Patch Detection**: 
   - If the input is a JSON **Object**, it is treated as a Merge Patch.
   - If the input is a JSON **Array**, it is treated as a JSON Patch.
4. **Garbage Collection**: The store keeps a history of generations. When the oldest generation's `Arc` count drops to 1, it is automatically removed.
5. **Rebase**: Every `interval` generations (default 20), the store performs a deep clone of the latest value to break references to old structural fragments and ensure memory locality.

## License

This project is licensed under the MIT License or Apache 2.0.
