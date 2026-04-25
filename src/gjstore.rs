use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::sync::Arc;

use parking_lot::RwLock;
// use serde_json::json;
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

/// The internal generational logic
pub struct Store {
    history: VecDeque<(usize, Arc<SharedValue>)>,
    next_gen: usize,
}

impl Store {
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
        // Clone latest value (in a block to ensure latest_arc is dropped before GC)
        let mut new_val = {
            // 1. Get latest, clone the Arc to start COW process
            let latest_arc = self.history.back().unwrap().1.clone();
            (*latest_arc).clone()
        };

        // Apply patch (efficient structural update)
        new_val.apply_merge_patch(patch);

        // Periodic Rebase (Defragmentation for read performance)
        let final_value = if self.next_gen.is_multiple_of(20) {
            Arc::new(new_val.deep_clone())
        } else {
            Arc::new(new_val)
        };

        let next = self.next_gen;
        self.history.push_back((next, final_value));
        self.next_gen += 1;

        // Automatic GC: Drop old versions if no clients are using them
        while self.history.len() > 1 {
            // Check if only the history deque holds this Arc
            if Arc::strong_count(&self.history[0].1) == 1 {
                self.history.pop_front();
            } else {
                break; // A client is still holding this version
            }
        }
    }

    /// Retrieve a specific generation. Returns Arc for O(1) handover.
    pub fn get(&self, generation: usize) -> Option<Arc<SharedValue>> {
        self.history
            .iter()
            .find(|(id, _)| *id == generation)
            .map(|(_, v)| Arc::clone(v))
    }
}

/// The Thread-Safe Store
pub struct SharedStore {
    inner: Arc<RwLock<Store>>,
}

impl SharedStore {
    pub fn new(initial_json: Value) -> Self {
        Self {
            inner: Arc::new(RwLock::new(Store::new(initial_json))),
        }
    }

    /// Apply a patch. Performs COW update, periodic rebase, and automatic GC.
    pub fn update(&self, patch: Value) {
        let mut store = self.inner.write();
        store.update(patch)
    }

    /// Retrieve a specific generation. Returns Arc for O(1) handover.
    pub fn get(&self, generation: usize) -> Option<Arc<SharedValue>> {
        let store = self.inner.read();
        store.get(generation)
    }
}
