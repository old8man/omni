//! Buddy/Companion System
//!
//! A deterministic companion creature generated from the user's ID.
//! Each user gets a unique companion with species, stats, appearance,
//! and personality -- all derived from a seeded PRNG so the same user
//! always gets the same companion.

pub mod companion;
pub mod prompt;
pub mod sprites;
pub mod types;

pub use companion::{get_companion, roll, roll_with_seed, Roll};
pub use types::{
    Companion, CompanionBones, CompanionSoul, Eye, Hat, Rarity, Species, StatName,
    StoredCompanion,
};
