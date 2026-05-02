use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::sync::Arc;

use parking_lot::Mutex;
use parking_lot::RwLock;
use serde_json::Value;

/// A structural-sharing JSON value.
/// Containers (Objects/Arrays) are wrapped in Arc to allow different generations
/// to share the same memory for untouched branches.
#[derive(Clone, Debug)]
pub enum SharedValue {
    Object(Arc<BTreeMap<String, SharedValue>>),
    Array(Arc<Vec<SharedValue>>),
    Leaf(Value),
}

impl From<Value> for SharedValue {
    fn from(value: Value) -> Self {
        match value {
            Value::Object(m) => {
                let shared_m = m
                    .into_iter()
                    .map(|(k, v)| (k, SharedValue::from(v)))
                    .collect();
                SharedValue::Object(Arc::new(shared_m))
            }
            Value::Array(a) => {
                let shared_a = a.into_iter().map(SharedValue::from).collect();
                SharedValue::Array(Arc::new(shared_a))
            }
            leaf => SharedValue::Leaf(leaf),
        }
    }
}

impl SharedValue {
    /// Recursively clones every node to ensure memory locality and
    /// disconnect from previous generations. Use in re-basing.
    fn deep_clone(&self) -> Self {
        match self {
            SharedValue::Leaf(l) => SharedValue::Leaf(l.clone()),
            SharedValue::Array(a) => {
                let new_vec = a.iter().map(|v| v.deep_clone()).collect();
                SharedValue::Array(Arc::new(new_vec))
            }
            SharedValue::Object(m) => {
                let new_map = m.iter().map(|(k, v)| (k.clone(), v.deep_clone())).collect();
                SharedValue::Object(Arc::new(new_map))
            }
        }
    }

    /// RFC 7396 Merge Patch implementation with structural sharing.
    fn apply_merge_patch(&mut self, patch: Value) {
        match patch {
            Value::Object(patch_map) => {
                // If current target isn't an object, replace it with an empty one first
                if !matches!(self, SharedValue::Object(_)) {
                    *self = SharedValue::Object(Arc::new(BTreeMap::new()));
                }

                if let SharedValue::Object(current_arc) = self {
                    // COW: Only clones the BTreeMap if other generations still hold it
                    let map = Arc::make_mut(current_arc);
                    for (key, value) in patch_map {
                        if value.is_null() {
                            map.remove(&key);
                        } else if let Some(child) = map.get_mut(&key) {
                            child.apply_merge_patch(value);
                        } else {
                            map.insert(key, SharedValue::from(value));
                        }
                    }
                }
            }
            // Non-object patches (scalars/arrays) replace the target entirely
            other => {
                *self = SharedValue::from(other);
            }
        }
    }
}

/// The Store
#[derive(Clone, Debug)]
pub struct Store {
    history: VecDeque<(usize, Arc<SharedValue>)>,
    next_gen: usize,
}

impl Store {
    /// Create a store from an initial JSON Value.
    pub fn new(initial_json: Value) -> Self {
        let initial_shared = Arc::new(initial_json.into());
        let mut history = VecDeque::new();
        history.push_back((0, initial_shared));

        Self {
            history,
            next_gen: 1,
        }
    }

    /// Apply a patch. Performs COW update, periodic rebase, and automatic GC.
    pub fn update(&mut self, patch: Value) {
        // Use a block to ensure latest_arc is dropped BEFORE gc
        let (latest_gen, mut new_val) = {
            let (latest_gen, latest_arc) = self.latest_with_gen();

            (latest_gen, (*latest_arc).clone())
        };

        // Apply patch (efficient structural update)
        new_val.apply_merge_patch(patch);

        let next_gen = latest_gen + 1;
        let final_value = if next_gen.is_multiple_of(20) {
            Arc::new(new_val.deep_clone())
        } else {
            Arc::new(new_val)
        };

        self.commit(next_gen, final_value);
        self.gc();
    }

    /// Retrieve a specific generation. Returns Arc for O(1) handover.
    pub fn get(&self, generation: usize) -> Option<Arc<SharedValue>> {
        self.history
            .iter()
            .find(|(id, _)| *id == generation)
            .map(|(_, v)| Arc::clone(v))
    }

    /// Retrieve latest generation. Returns Arc for O(1) handover.
    pub fn latest(&self) -> Option<Arc<SharedValue>> {
        self.history.back().map(|(_, v)| Arc::clone(v))
    }

    /// Retrieve oldest generation. Returns Arc for O(1) handover.
    pub fn oldest(&self) -> Option<Arc<SharedValue>> {
        self.history.front().map(|(_, v)| Arc::clone(v))
    }

    /// Internal helper for SharedStore staged updates.
    fn latest_with_gen(&self) -> (usize, Arc<SharedValue>) {
        let (generation, val) = self.history.back().unwrap();
        (*generation, Arc::clone(val))
    }

    /// Internal helper for SharedStore staged updates.
    fn commit(&mut self, generation: usize, value: Arc<SharedValue>) {
        self.history.push_back((generation, value));
        self.next_gen = generation + 1;
    }

    /// Internal helper for SharedStore staged updates.
    fn gc(&mut self) {
        while self.history.len() > 1 {
            if Arc::strong_count(&self.history[0].1) == 1 {
                self.history.pop_front();
            } else {
                break;
            }
        }
    }

    #[cfg(feature = "bench-utils")]
    pub fn count_unique_nodes(&self) -> usize {
        use std::collections::HashSet;
        let mut seen_objects = HashSet::new();
        let mut seen_arrays = HashSet::new();
        let mut total_nodes = 0;

        for (_, sv) in &self.history {
            Self::count_unique_nodes_recursive(
                sv,
                &mut seen_objects,
                &mut seen_arrays,
                &mut total_nodes,
            );
        }
        total_nodes
    }

    #[cfg(feature = "bench-utils")]
    fn count_unique_nodes_recursive(
        sv: &SharedValue,
        seen_objects: &mut std::collections::HashSet<*const BTreeMap<String, SharedValue>>,
        seen_arrays: &mut std::collections::HashSet<*const Vec<SharedValue>>,
        total_nodes: &mut usize,
    ) {
        match sv {
            SharedValue::Leaf(_) => {
                *total_nodes += 1;
            }
            SharedValue::Object(m) => {
                let ptr = Arc::as_ptr(m);
                if seen_objects.insert(ptr) {
                    *total_nodes += 1;
                    for v in m.values() {
                        Self::count_unique_nodes_recursive(
                            v,
                            seen_objects,
                            seen_arrays,
                            total_nodes,
                        );
                    }
                }
            }
            SharedValue::Array(a) => {
                let ptr = Arc::as_ptr(a);
                if seen_arrays.insert(ptr) {
                    *total_nodes += 1;
                    for v in a.iter() {
                        Self::count_unique_nodes_recursive(
                            v,
                            seen_objects,
                            seen_arrays,
                            total_nodes,
                        );
                    }
                }
            }
        }
    }
}

/// The Thread-Safe Store
pub struct SharedStore {
    inner: Arc<SharedStoreInner>,
}

struct SharedStoreInner {
    store: RwLock<Store>,
    /// Serializes updates to ensure only one thread is calculating the next generation
    /// while still allowing concurrent readers.
    update_lock: Mutex<()>,
}

impl SharedStore {
    /// Create a store from an initial JSON Value.
    pub fn new(initial_json: Value) -> Self {
        Self {
            inner: Arc::new(SharedStoreInner {
                store: RwLock::new(Store::new(initial_json)),
                update_lock: Mutex::new(()),
            }),
        }
    }

    /// Apply a patch. Performs COW update and periodic rebase outside the write lock.
    /// Serialization is handled by a Mutex, but the RwLock is only held for a brief moment to commit.
    pub fn update(&self, patch: Value) {
        // Serialize writers to ensure we don't have multiple threads trying to calculate Gen N
        let _guard = self.inner.update_lock.lock();

        // Get the baseline under a READ lock (very brief)
        // Use a block to ensure latest_arc is dropped BEFORE gc
        let (latest_gen, mut new_val) = {
            let (latest_gen, latest_arc) = {
                let store = self.inner.store.read();
                store.latest_with_gen()
            };

            (latest_gen, (*latest_arc).clone())
        };

        // Do most of our modification work without locking the store
        new_val.apply_merge_patch(patch);

        let next_gen = latest_gen + 1;
        let final_value = if next_gen.is_multiple_of(20) {
            Arc::new(new_val.deep_clone())
        } else {
            Arc::new(new_val)
        };

        // Finally, commit the result under a WRITE lock
        {
            let mut store = self.inner.store.write();
            store.commit(next_gen, final_value);
            store.gc();
        }
    }

    /// Retrieve a specific generation. Returns Arc for O(1) handover.
    pub fn get(&self, generation: usize) -> Option<Arc<SharedValue>> {
        let store = self.inner.store.read();
        store.get(generation)
    }

    /// Retrieve latest generation. Returns Arc for O(1) handover.
    pub fn latest(&self) -> Option<Arc<SharedValue>> {
        let store = self.inner.store.read();
        store.latest()
    }

    /// Retrieve oldest generation. Returns Arc for O(1) handover.
    pub fn oldest(&self) -> Option<Arc<SharedValue>> {
        let store = self.inner.store.read();
        store.oldest()
    }
}
