//! Type definitions for the Buddy/Companion system.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

// ---------------------------------------------------------------------------
// Species (18 total)
// ---------------------------------------------------------------------------

/// All 18 companion species, ordered for array indexing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Species {
    Duck,
    Goose,
    Blob,
    Cat,
    Dragon,
    Octopus,
    Owl,
    Penguin,
    Turtle,
    Snail,
    Ghost,
    Axolotl,
    Capybara,
    Cactus,
    Robot,
    Rabbit,
    Mushroom,
    Chonk,
}

impl fmt::Display for Species {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Duck => "duck",
            Self::Goose => "goose",
            Self::Blob => "blob",
            Self::Cat => "cat",
            Self::Dragon => "dragon",
            Self::Octopus => "octopus",
            Self::Owl => "owl",
            Self::Penguin => "penguin",
            Self::Turtle => "turtle",
            Self::Snail => "snail",
            Self::Ghost => "ghost",
            Self::Axolotl => "axolotl",
            Self::Capybara => "capybara",
            Self::Cactus => "cactus",
            Self::Robot => "robot",
            Self::Rabbit => "rabbit",
            Self::Mushroom => "mushroom",
            Self::Chonk => "chonk",
        };
        write!(f, "{name}")
    }
}

/// All species in array form, for indexing by the PRNG.
pub const SPECIES: &[Species] = &[
    Species::Duck,
    Species::Goose,
    Species::Blob,
    Species::Cat,
    Species::Dragon,
    Species::Octopus,
    Species::Owl,
    Species::Penguin,
    Species::Turtle,
    Species::Snail,
    Species::Ghost,
    Species::Axolotl,
    Species::Capybara,
    Species::Cactus,
    Species::Robot,
    Species::Rabbit,
    Species::Mushroom,
    Species::Chonk,
];

// ---------------------------------------------------------------------------
// Rarity
// ---------------------------------------------------------------------------

/// Companion rarity tiers.
/// Distribution: Common 60%, Uncommon 25%, Rare 10%, Epic 4%, Legendary 1%.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rarity {
    Common,
    Uncommon,
    Rare,
    Epic,
    Legendary,
}

impl fmt::Display for Rarity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Common => "common",
            Self::Uncommon => "uncommon",
            Self::Rare => "rare",
            Self::Epic => "epic",
            Self::Legendary => "legendary",
        };
        write!(f, "{name}")
    }
}

/// Rarity tiers in order, for weighted roll iteration.
pub const RARITIES: &[Rarity] = &[
    Rarity::Common,
    Rarity::Uncommon,
    Rarity::Rare,
    Rarity::Epic,
    Rarity::Legendary,
];

/// Rarity weights for the weighted random roll (out of 100).
pub fn rarity_weight(rarity: Rarity) -> u32 {
    match rarity {
        Rarity::Common => 60,
        Rarity::Uncommon => 25,
        Rarity::Rare => 10,
        Rarity::Epic => 4,
        Rarity::Legendary => 1,
    }
}

/// Star display for each rarity.
pub fn rarity_stars(rarity: Rarity) -> &'static str {
    match rarity {
        Rarity::Common => "\u{2605}",
        Rarity::Uncommon => "\u{2605}\u{2605}",
        Rarity::Rare => "\u{2605}\u{2605}\u{2605}",
        Rarity::Epic => "\u{2605}\u{2605}\u{2605}\u{2605}",
        Rarity::Legendary => "\u{2605}\u{2605}\u{2605}\u{2605}\u{2605}",
    }
}

/// Minimum stat floor per rarity tier.
pub fn rarity_floor(rarity: Rarity) -> u32 {
    match rarity {
        Rarity::Common => 5,
        Rarity::Uncommon => 15,
        Rarity::Rare => 25,
        Rarity::Epic => 35,
        Rarity::Legendary => 50,
    }
}

// ---------------------------------------------------------------------------
// Eyes (6 styles)
// ---------------------------------------------------------------------------

/// Eye style for the companion sprite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Eye {
    #[serde(rename = "\u{00B7}")]
    Dot,       // ·
    #[serde(rename = "\u{2726}")]
    Star,      // ✦
    #[serde(rename = "\u{00D7}")]
    Cross,     // ×
    #[serde(rename = "\u{25C9}")]
    Circle,    // ◉
    #[serde(rename = "@")]
    At,        // @
    #[serde(rename = "\u{00B0}")]
    Degree,    // °
}

impl Eye {
    /// Get the character representation of this eye style.
    pub fn as_char(&self) -> char {
        match self {
            Self::Dot => '\u{00B7}',     // ·
            Self::Star => '\u{2726}',    // ✦
            Self::Cross => '\u{00D7}',   // ×
            Self::Circle => '\u{25C9}',  // ◉
            Self::At => '@',
            Self::Degree => '\u{00B0}',  // °
        }
    }

    /// Get the string representation of this eye style.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Dot => "\u{00B7}",
            Self::Star => "\u{2726}",
            Self::Cross => "\u{00D7}",
            Self::Circle => "\u{25C9}",
            Self::At => "@",
            Self::Degree => "\u{00B0}",
        }
    }
}

impl fmt::Display for Eye {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_char())
    }
}

/// All eye styles in array form, for indexing by the PRNG.
pub const EYES: &[Eye] = &[
    Eye::Dot,
    Eye::Star,
    Eye::Cross,
    Eye::Circle,
    Eye::At,
    Eye::Degree,
];

// ---------------------------------------------------------------------------
// Hats (8 styles)
// ---------------------------------------------------------------------------

/// Hat style for the companion sprite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hat {
    None,
    Crown,
    Tophat,
    Propeller,
    Halo,
    Wizard,
    Beanie,
    Tinyduck,
}

impl fmt::Display for Hat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::None => "none",
            Self::Crown => "crown",
            Self::Tophat => "tophat",
            Self::Propeller => "propeller",
            Self::Halo => "halo",
            Self::Wizard => "wizard",
            Self::Beanie => "beanie",
            Self::Tinyduck => "tinyduck",
        };
        write!(f, "{name}")
    }
}

/// All hat styles in array form, for indexing by the PRNG.
pub const HATS: &[Hat] = &[
    Hat::None,
    Hat::Crown,
    Hat::Tophat,
    Hat::Propeller,
    Hat::Halo,
    Hat::Wizard,
    Hat::Beanie,
    Hat::Tinyduck,
];

// ---------------------------------------------------------------------------
// Stats (5 procedural stats, 1-100)
// ---------------------------------------------------------------------------

/// Companion stat names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum StatName {
    Debugging,
    Patience,
    Chaos,
    Wisdom,
    Snark,
}

impl fmt::Display for StatName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Debugging => "DEBUGGING",
            Self::Patience => "PATIENCE",
            Self::Chaos => "CHAOS",
            Self::Wisdom => "WISDOM",
            Self::Snark => "SNARK",
        };
        write!(f, "{name}")
    }
}

/// All stat names in array form, for indexing by the PRNG.
pub const STAT_NAMES: &[StatName] = &[
    StatName::Debugging,
    StatName::Patience,
    StatName::Chaos,
    StatName::Wisdom,
    StatName::Snark,
];

/// Stat values for a companion (maps each stat name to a value 1-100).
pub type CompanionStats = HashMap<StatName, u32>;

// ---------------------------------------------------------------------------
// Appearance
// ---------------------------------------------------------------------------

/// Visual appearance traits of a companion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Appearance {
    pub eye_style: Eye,
    pub hat: Hat,
    /// Base color index (used for shiny variant rendering).
    pub color: u32,
}

// ---------------------------------------------------------------------------
// Companion structs
// ---------------------------------------------------------------------------

/// Deterministic "bones" -- derived from hash(userId), never persisted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompanionBones {
    pub rarity: Rarity,
    pub species: Species,
    pub eye: Eye,
    pub hat: Hat,
    pub shiny: bool,
    pub stats: CompanionStats,
}

/// Model-generated "soul" -- stored in config after first hatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompanionSoul {
    pub name: String,
    pub personality: String,
}

/// A fully-realized companion (bones + soul + hatch timestamp).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Companion {
    // Bones
    pub rarity: Rarity,
    pub species: Species,
    pub eye: Eye,
    pub hat: Hat,
    pub shiny: bool,
    pub stats: CompanionStats,

    // Soul
    pub name: String,
    pub personality: String,

    // State
    pub hatched_at: u64,
}

impl Companion {
    /// Create a `Companion` by merging bones with a stored soul.
    pub fn from_bones_and_soul(
        bones: &CompanionBones,
        soul: &CompanionSoul,
        hatched_at: u64,
    ) -> Self {
        Self {
            rarity: bones.rarity,
            species: bones.species,
            eye: bones.eye,
            hat: bones.hat,
            shiny: bones.shiny,
            stats: bones.stats.clone(),
            name: soul.name.clone(),
            personality: soul.personality.clone(),
            hatched_at,
        }
    }

    /// Extract the bones from this companion.
    pub fn bones(&self) -> CompanionBones {
        CompanionBones {
            rarity: self.rarity,
            species: self.species,
            eye: self.eye,
            hat: self.hat,
            shiny: self.shiny,
            stats: self.stats.clone(),
        }
    }
}

/// What actually persists in config. Bones are regenerated from hash(userId)
/// on every read so species renames don't break stored companions and users
/// can't edit their way to a legendary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCompanion {
    pub name: String,
    pub personality: String,
    pub hatched_at: u64,
}

impl From<&Companion> for StoredCompanion {
    fn from(c: &Companion) -> Self {
        Self {
            name: c.name.clone(),
            personality: c.personality.clone(),
            hatched_at: c.hatched_at,
        }
    }
}
