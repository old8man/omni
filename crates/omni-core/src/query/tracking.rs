use uuid::Uuid;

/// Per-query chain tracking for conversation continuity.
///
/// `chain_id` is a UUID that stays the same for an entire conversation.
/// `depth` increments each turn within the conversation.
#[derive(Clone, Debug)]
pub struct QueryTracking {
    /// Unique ID for the entire conversation chain. Stays constant across
    /// all turns in a conversation.
    pub chain_id: String,
    /// Increments each turn. Starts at 0 for the first turn.
    pub depth: u64,
}

impl QueryTracking {
    /// Create a new tracking context for a fresh conversation.
    pub fn new() -> Self {
        Self {
            chain_id: Uuid::new_v4().to_string(),
            depth: 0,
        }
    }

    /// Create a tracking context for the next turn in an existing chain.
    pub fn next_turn(&self) -> Self {
        Self {
            chain_id: self.chain_id.clone(),
            depth: self.depth + 1,
        }
    }

    /// Initialize or advance tracking. If `previous` is `None`, creates a new
    /// chain; otherwise increments the depth.
    pub fn init_or_advance(previous: Option<&QueryTracking>) -> Self {
        match previous {
            Some(prev) => prev.next_turn(),
            None => Self::new(),
        }
    }
}

impl Default for QueryTracking {
    fn default() -> Self {
        Self::new()
    }
}
