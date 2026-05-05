//! A Generational Json Store
//!
//! Supports both RFC 7396 (JSON Merge Patch) and RFC 6902 (JSON Patch) for generational updates.
//!
//! Create a store and then apply updates. Each update results in a new generation. Structural
//! sharing is performed, so that memory footprints are kept low.
//!
//! The `update` method automatically detects the patch format:
//! - **Object**: Treated as an RFC 7396 Merge Patch.
//! - **Array**: Treated as an RFC 6902 JSON Patch.
//!
//! Two features co-operate to try to keep memory footprints low:
//!
//!  - periodic deep copies from history ensure that long-lived objects don't impede garbage collection
//!  - garbage collection attempts to delete older generations which are no longer referenced

pub mod gjstore;

#[cfg(test)]
mod tests;
