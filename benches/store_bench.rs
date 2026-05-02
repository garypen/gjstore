use criterion::{Criterion, criterion_group, criterion_main};
use gjstore::gjstore::Store;
use json_patch::merge;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde_json::{Map, Value, json};
use std::collections::VecDeque;
use std::sync::Arc;

/// NaiveStore: Clones and updates the entire store for each generation using standard Value.
pub struct NaiveStore {
    history: VecDeque<(usize, Arc<Value>)>,
    next_gen: usize,
    history_limit: usize,
}

impl NaiveStore {
    pub fn new(initial_json: Value, history_limit: usize) -> Self {
        let mut history = VecDeque::new();
        history.push_back((0, Arc::new(initial_json)));
        Self {
            history,
            next_gen: 1,
            history_limit,
        }
    }

    pub fn update(&mut self, patch: Value) {
        let latest_arc = self.history.back().unwrap().1.clone();
        let mut new_val = (*latest_arc).clone(); // Full deep clone
        merge(&mut new_val, &patch);
        self.history.push_back((self.next_gen, Arc::new(new_val)));
        self.next_gen += 1;

        while self.history.len() > self.history_limit {
            if Arc::strong_count(&self.history[0].1) == 1 {
                self.history.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn latest(&self) -> Option<Arc<Value>> {
        self.history.back().map(|(_, v)| Arc::clone(v))
    }

    pub fn count_total_nodes(&self) -> usize {
        self.history.iter().map(|(_, v)| count_nodes_value(v)).sum()
    }
}

fn count_nodes_value(v: &Value) -> usize {
    match v {
        Value::Object(m) => 1 + m.values().map(count_nodes_value).sum::<usize>(),
        Value::Array(a) => 1 + a.iter().map(count_nodes_value).sum::<usize>(),
        _ => 1,
    }
}

#[derive(Clone, Copy, Debug)]
enum JsonType {
    Object,
    Array,
    Leaf,
}

struct PathInfo {
    path: Vec<String>,
    json_type: JsonType,
}

fn collect_typed_paths(v: &Value, p: Vec<String>, paths: &mut Vec<PathInfo>) {
    if let Value::Object(m) = v {
        for (k, child) in m {
            let mut next_p = p.clone();
            next_p.push(k.clone());

            let json_type = match child {
                Value::Object(_) => JsonType::Object,
                Value::Array(_) => JsonType::Array,
                _ => JsonType::Leaf,
            };

            paths.push(PathInfo {
                path: next_p.clone(),
                json_type,
            });

            collect_typed_paths(child, next_p, paths);
        }
    }
}

fn generate_patch_pool(rng: &mut ChaCha8Rng, paths: &[PathInfo], pool_size: usize) -> Vec<Value> {
    (0..pool_size)
        .map(|_| {
            let mut patch = json!({});
            // Apply 3 random updates per patch
            for _ in 0..3 {
                if paths.is_empty() {
                    break;
                }
                let info = &paths[rng.gen_range(0..paths.len())];

                let val = match info.json_type {
                    JsonType::Leaf => match rng.gen_range(0..3) {
                        0 => Value::Number(rng.gen_range(0..1000).into()),
                        1 => Value::String(format!("patch_v_{}", rng.gen_range(0..1000))),
                        _ => Value::Bool(rng.r#gen()),
                    },
                    JsonType::Object => {
                        // Generate a small replacement/merge object (2-4 nodes)
                        let mut remaining = rng.gen_range(2..5);
                        generate_random_json_recursive(rng, 0, &mut remaining, true)
                    }
                    JsonType::Array => {
                        // Generate a small replacement array
                        let mut arr = Vec::new();
                        for _ in 0..rng.gen_range(2..5) {
                            arr.push(json!(rng.gen_range(0..100)));
                        }
                        Value::Array(arr)
                    }
                };

                let mut curr = &mut patch;
                for (i, seg) in info.path.iter().enumerate() {
                    if i == info.path.len() - 1 {
                        if let Value::Object(m) = curr {
                            m.insert(seg.clone(), val.clone());
                        }
                    } else if let Value::Object(m) = curr {
                        curr = m.entry(seg.clone()).or_insert(json!({}));
                    }
                }
            }
            patch
        })
        .collect()
}

fn generate_random_json_recursive(
    rng: &mut ChaCha8Rng,
    depth: usize,
    remaining: &mut usize,
    force_object: bool,
) -> Value {
    if *remaining == 0 {
        return Value::Bool(true);
    }
    *remaining -= 1;

    let p_container = match depth {
        0..=3 => 0.9,
        4..=6 => 0.4,
        _ => 0.05,
    };

    if (force_object || rng.gen_bool(p_container)) && *remaining > 0 {
        if force_object || rng.gen_bool(0.7) {
            let mut map = Map::new();
            let children = rng.gen_range(2..6);
            for i in 0..children {
                if *remaining == 0 {
                    break;
                }
                map.insert(
                    format!("k_{}_{}", depth, i),
                    generate_random_json_recursive(rng, depth + 1, remaining, false),
                );
            }
            Value::Object(map)
        } else {
            let mut arr = Vec::new();
            let children = rng.gen_range(2..6);
            for _ in 0..children {
                if *remaining == 0 {
                    break;
                }
                arr.push(generate_random_json_recursive(
                    rng,
                    depth + 1,
                    remaining,
                    false,
                ));
            }
            Value::Array(arr)
        }
    } else {
        match rng.gen_range(0..3) {
            0 => Value::Number(rng.gen_range(0..1000).into()),
            1 => Value::String(format!("v_{}", rng.gen_range(0..1000))),
            _ => Value::Bool(rng.r#gen()),
        }
    }
}

fn generate_random_json(rng: &mut ChaCha8Rng, target_nodes: usize) -> Value {
    let mut remaining = target_nodes;
    generate_random_json_recursive(rng, 0, &mut remaining, true)
}

fn report_memory(corpus: &Value, patch_pool: &[Value], history_limit: usize, iterations: usize) {
    println!(
        "\n--- Memory Usage Report (History Limit: {}) ---",
        history_limit
    );
    let mut store = Store::new(corpus.clone());
    let mut naive = NaiveStore::new(corpus.clone(), history_limit);

    println!("Before updates:");
    println!("Store Unique Nodes: {}", store.count_unique_nodes());
    println!("Naive Total Nodes : {}", naive.count_total_nodes());
    println!();
    let mut _garbage = store.oldest();
    for i in 1..iterations {
        // Keep moving forward garbage reference
        if i >= history_limit {
            _garbage = store.get(i - history_limit);
        }
        let patch = patch_pool[i % patch_pool.len()].clone();
        store.update(patch.clone());
        naive.update(patch);
        if i == history_limit {
            println!("At History Limit:");
            println!("Store Unique Nodes: {}", store.count_unique_nodes());
            println!("Naive Total Nodes : {}", naive.count_total_nodes());
            println!();
        }
    }

    println!("After {} updates:", iterations);
    println!("Store Unique Nodes: {}", store.count_unique_nodes());
    println!("Naive Total Nodes : {}", naive.count_total_nodes());
    println!("-----------------------------------------------\n");
}

pub fn criterion_benchmark(c: &mut Criterion) {
    let mut rng = ChaCha8Rng::seed_from_u64(42);

    println!("Generating corpus (10k nodes)...");
    let corpus = generate_random_json(&mut rng, 10_000);

    let mut paths = Vec::new();
    collect_typed_paths(&corpus, Vec::new(), &mut paths);

    let patch_pool = generate_patch_pool(&mut rng, &paths, 1000);

    let history_limit = 10;

    // Report memory usage once for a fixed scenario
    report_memory(&corpus, &patch_pool, history_limit, 50);

    let ratios = [100, 1000, 10000];

    for read_ratio in ratios {
        let group_name = format!("Mixed Workload (1 Write : {} Reads)", read_ratio);
        let mut group = c.benchmark_group(group_name);

        group.bench_function("Store", |b| {
            let mut store = Store::new(corpus.clone());
            let mut garbage = store.oldest();
            let mut idx = 0;
            b.iter(|| {
                // 1 Write
                store.update(patch_pool[idx % 1000].clone());
                idx += 1;

                // N Reads
                let mut _generation;
                for _ in 0..read_ratio {
                    _generation = store.latest().unwrap();
                }
                if idx.is_multiple_of(history_limit) && idx > 0 {
                    garbage = store.get(idx - history_limit);
                }
            });
        });

        group.bench_function("Naive", |b| {
            let mut store = NaiveStore::new(corpus.clone(), history_limit);
            let mut idx = 0;
            b.iter(|| {
                // 1 Write
                store.update(patch_pool[idx % 1000].clone());
                idx += 1;

                // N Reads
                let mut _generation;
                for _ in 0..read_ratio {
                    _generation = store.latest().unwrap();
                }
            });
        });
        group.finish();
    }
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
