use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::sync::Arc;

use bon::bon;
use json_patch::Patch;
use parking_lot::Mutex;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("JSON Patch error: {0}")]
    PatchError(String),
    #[error("Invalid patch format: expected Object (Merge Patch) or Array (JSON Patch)")]
    InvalidPatchFormat,
}

/// A structural-sharing JSON value.
/// Containers (Objects/Arrays) are wrapped in Arc to allow different generations
/// to share the same memory for untouched branches.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
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

impl From<SharedValue> for Value {
    fn from(shared: SharedValue) -> Self {
        match shared {
            SharedValue::Leaf(v) => v,
            SharedValue::Array(a) => Value::Array(a.iter().cloned().map(Value::from).collect()),
            SharedValue::Object(m) => Value::Object(
                m.iter()
                    .map(|(k, v)| (k.clone(), Value::from(v.clone())))
                    .collect(),
            ),
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

    /// RFC 6902 JSON Patch implementation with structural sharing.
    fn apply_json_patch(&mut self, patch: Patch) -> Result<(), StoreError> {
        for op in patch.0 {
            self.apply_operation(op)?;
        }
        Ok(())
    }

    fn apply_operation(&mut self, op: json_patch::PatchOperation) -> Result<(), StoreError> {
        match op {
            json_patch::PatchOperation::Add(add) => {
                let val = SharedValue::from(add.value);
                self.add_at(add.path.as_str(), val)?;
            }
            json_patch::PatchOperation::Remove(rem) => {
                self.remove_at(rem.path.as_str())?;
            }
            json_patch::PatchOperation::Replace(rep) => {
                let val = SharedValue::from(rep.value);
                let target = self.get_mut(rep.path.as_str())?;
                *target = val;
            }
            json_patch::PatchOperation::Move(mov) => {
                let val = self.remove_at(mov.from.as_str())?;
                self.add_at(mov.path.as_str(), val)?;
            }
            json_patch::PatchOperation::Copy(cop) => {
                let val = self.get_at(cop.from.as_str())?.clone();
                self.add_at(cop.path.as_str(), val)?;
            }
            json_patch::PatchOperation::Test(test) => {
                let expected = SharedValue::from(test.value);
                let actual = self.get_at(test.path.as_str())?;
                if !Self::equals(actual, &expected) {
                    return Err(StoreError::PatchError(format!(
                        "Test failed at path: {}",
                        test.path
                    )));
                }
            }
        }
        Ok(())
    }

    fn equals(a: &SharedValue, b: &SharedValue) -> bool {
        match (a, b) {
            (SharedValue::Leaf(v1), SharedValue::Leaf(v2)) => v1 == v2,
            (SharedValue::Array(a1), SharedValue::Array(a2)) => {
                if Arc::ptr_eq(a1, a2) {
                    return true;
                }
                if a1.len() != a2.len() {
                    return false;
                }
                a1.iter().zip(a2.iter()).all(|(x, y)| Self::equals(x, y))
            }
            (SharedValue::Object(o1), SharedValue::Object(o2)) => {
                if Arc::ptr_eq(o1, o2) {
                    return true;
                }
                if o1.len() != o2.len() {
                    return false;
                }
                o1.iter()
                    .zip(o2.iter())
                    .all(|((k1, v1), (k2, v2))| k1 == k2 && Self::equals(v1, v2))
            }
            _ => false,
        }
    }

    fn get_at(&self, path: &str) -> Result<&SharedValue, StoreError> {
        let mut current = self;
        for segment in parse_segments(path) {
            match current {
                SharedValue::Object(m) => {
                    current = m.get(&segment).ok_or_else(|| {
                        StoreError::PatchError(format!("Path not found: {} (at {})", path, segment))
                    })?;
                }
                SharedValue::Array(a) => {
                    let idx = parse_index(&segment, a.len(), false)?;
                    current = a.get(idx).ok_or_else(|| {
                        StoreError::PatchError(format!(
                            "Index out of bounds: {} (at {})",
                            path, segment
                        ))
                    })?;
                }
                SharedValue::Leaf(_) => {
                    return Err(StoreError::PatchError(format!(
                        "Cannot navigate into leaf at {}",
                        segment
                    )));
                }
            }
        }
        Ok(current)
    }

    fn get_mut(&mut self, path: &str) -> Result<&mut SharedValue, StoreError> {
        let mut current = self;
        for segment in parse_segments(path) {
            match current {
                SharedValue::Object(arc) => {
                    let map = Arc::make_mut(arc);
                    current = map.get_mut(&segment).ok_or_else(|| {
                        StoreError::PatchError(format!("Path not found: {} (at {})", path, segment))
                    })?;
                }
                SharedValue::Array(arc) => {
                    let vec = Arc::make_mut(arc);
                    let idx = parse_index(&segment, vec.len(), false)?;
                    current = vec.get_mut(idx).ok_or_else(|| {
                        StoreError::PatchError(format!(
                            "Index out of bounds: {} (at {})",
                            path, segment
                        ))
                    })?;
                }
                SharedValue::Leaf(_) => {
                    return Err(StoreError::PatchError(format!(
                        "Cannot navigate into leaf at {}",
                        segment
                    )));
                }
            }
        }
        Ok(current)
    }

    fn add_at(&mut self, path: &str, value: SharedValue) -> Result<(), StoreError> {
        if path.is_empty() {
            *self = value;
            return Ok(());
        }

        let (parent_path, last_segment) = split_path(path);
        let parent = self.get_mut(parent_path)?;

        match parent {
            SharedValue::Object(arc) => {
                let map = Arc::make_mut(arc);
                map.insert(last_segment.to_string(), value);
            }
            SharedValue::Array(arc) => {
                let vec = Arc::make_mut(arc);
                let idx = parse_index(last_segment, vec.len(), true)?;
                if idx >= vec.len() {
                    vec.push(value);
                } else {
                    vec.insert(idx, value);
                }
            }
            SharedValue::Leaf(_) => {
                return Err(StoreError::PatchError(format!(
                    "Cannot add to leaf at {}",
                    parent_path
                )));
            }
        }
        Ok(())
    }

    fn remove_at(&mut self, path: &str) -> Result<SharedValue, StoreError> {
        if path.is_empty() {
            return Err(StoreError::PatchError("Cannot remove root".into()));
        }

        let (parent_path, last_segment) = split_path(path);
        let parent = self.get_mut(parent_path)?;

        match parent {
            SharedValue::Object(arc) => {
                let map = Arc::make_mut(arc);
                map.remove(last_segment).ok_or_else(|| {
                    StoreError::PatchError(format!("Key not found: {}", last_segment))
                })
            }
            SharedValue::Array(arc) => {
                let vec = Arc::make_mut(arc);
                let idx = parse_index(last_segment, vec.len(), false)?;
                if idx >= vec.len() {
                    return Err(StoreError::PatchError(format!(
                        "Index out of bounds: {}",
                        last_segment
                    )));
                }
                Ok(vec.remove(idx))
            }
            SharedValue::Leaf(_) => Err(StoreError::PatchError(format!(
                "Cannot remove from leaf at {}",
                parent_path
            ))),
        }
    }
}

fn parse_segments(path: &str) -> impl Iterator<Item = String> + '_ {
    path.split('/')
        .skip(1)
        .map(|s| s.replace("~1", "/").replace("~0", "~"))
}

fn split_path(path: &str) -> (&str, &str) {
    if let Some(pos) = path.rfind('/') {
        (&path[..pos], &path[pos + 1..])
    } else {
        ("", path)
    }
}

fn parse_index(segment: &str, len: usize, allow_end: bool) -> Result<usize, StoreError> {
    if segment == "-" {
        if allow_end {
            return Ok(len);
        } else {
            return Err(StoreError::PatchError("'-' index not allowed here".into()));
        }
    }

    // RFC 6901: array index must be a number with no leading zeros (unless it's just '0')
    if segment.len() > 1 && segment.starts_with('0') {
        return Err(StoreError::PatchError(format!(
            "Invalid array index (leading zero): {}",
            segment
        )));
    }

    let idx: usize = segment
        .parse()
        .map_err(|_| StoreError::PatchError(format!("Invalid array index: {}", segment)))?;

    Ok(idx)
}

/// The Store
#[derive(Clone, Debug)]
pub struct Store {
    history: VecDeque<(usize, Arc<SharedValue>)>,
    next_gen: usize,
    interval: usize,
}

#[bon]
impl Store {
    /// Create a store with optional initial JSON Value and rebase interval.
    #[builder]
    pub fn new(
        #[builder(default = Value::Object(Default::default()))] value: Value,
        #[builder(default = 20)] interval: usize,
    ) -> Self {
        let initial_shared = Arc::new(value.into());
        let mut history = VecDeque::new();
        history.push_back((0, initial_shared));

        Self {
            history,
            next_gen: 1,
            interval,
        }
    }

    /// Apply a patch. Performs COW update, periodic rebase, and automatic GC.
    pub fn update(&mut self, patch: Value) -> Result<(), StoreError> {
        // Use a block to ensure latest_arc is dropped BEFORE gc
        let (latest_gen, mut new_val) = {
            let (latest_gen, latest_arc) = self.latest_with_gen();

            (latest_gen, (*latest_arc).clone())
        };

        // Auto-detect patch type
        if patch.is_object() {
            new_val.apply_merge_patch(patch);
        } else if patch.is_array() {
            let p: Patch =
                serde_json::from_value(patch).map_err(|e| StoreError::PatchError(e.to_string()))?;
            new_val.apply_json_patch(p)?;
        } else {
            return Err(StoreError::InvalidPatchFormat);
        }

        let next_gen = latest_gen + 1;
        let final_value = if next_gen.is_multiple_of(self.interval) {
            Arc::new(new_val.deep_clone())
        } else {
            Arc::new(new_val)
        };

        self.commit(next_gen, final_value);
        self.gc();
        Ok(())
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
#[derive(Clone, Debug)]
pub struct SharedStore {
    inner: Arc<SharedStoreInner>,
}

#[derive(Debug)]
struct SharedStoreInner {
    store: RwLock<Store>,
    /// Serializes updates to ensure only one thread is calculating the next generation
    /// while still allowing concurrent readers.
    update_lock: Mutex<()>,
}

#[bon]
impl SharedStore {
    /// Create a store with optional initial JSON Value and rebase interval.
    #[builder]
    pub fn new(
        #[builder(default = Value::Object(Default::default()))] value: Value,
        #[builder(default = 20)] interval: usize,
    ) -> Self {
        Self {
            inner: Arc::new(SharedStoreInner {
                store: RwLock::new(Store::builder().value(value).interval(interval).build()),
                update_lock: Mutex::new(()),
            }),
        }
    }

    /// Apply a patch. Performs COW update and periodic rebase outside the write lock.
    /// Serialization is handled by a Mutex, but the RwLock is only held for a brief moment to commit.
    pub fn update(&self, patch: Value) -> Result<(), StoreError> {
        // Serialize writers to ensure we don't have multiple threads trying to calculate Gen N
        let _guard = self.inner.update_lock.lock();

        // Get the baseline under a READ lock (very brief)
        // Use a block to ensure latest_arc is dropped BEFORE gc
        let (latest_gen, interval, mut new_val) = {
            let (latest_gen, interval, latest_arc) = {
                let store = self.inner.store.read();
                let (generation, val) = store.latest_with_gen();
                (generation, store.interval, val)
            };

            (latest_gen, interval, (*latest_arc).clone())
        };

        // Do most of our modification work without locking the store
        // Auto-detect patch type
        if patch.is_object() {
            new_val.apply_merge_patch(patch);
        } else if patch.is_array() {
            let p: Patch =
                serde_json::from_value(patch).map_err(|e| StoreError::PatchError(e.to_string()))?;
            new_val.apply_json_patch(p)?;
        } else {
            return Err(StoreError::InvalidPatchFormat);
        }

        let next_gen = latest_gen + 1;
        let final_value = if next_gen.is_multiple_of(interval) {
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
        Ok(())
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
