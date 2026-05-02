//! A Generational Json Store
//!
//! Supports RFC 7396 Merge Patching for generational update
//!
//! Create a store and then apply updates. Each update results in a new generation. Structural
//! sharing is performed, so that memory footprints are kept low.
//!
//! Two features co-operate to try to keep memory footprints low:
//!
//!  - periodic deep copies from history ensure that long-lived objects don't impede garbage collection
//!  - garbage collection attempts to delete older generations which are no longer referenced

pub mod gjstore;

#[cfg(test)]
mod tests;
