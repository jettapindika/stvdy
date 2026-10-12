//! The pull cursor: its page size, its two kinds, and the staleness predicate.
//!
//! The cursor is a single integer because `seq` order equals commit order for one
//! user (`migrations/0006_sync_functions.sql` takes a per-user advisory lock before
//! the `change_log` insert), so a client can only ever observe a contiguous prefix.
//! The two things this module adds to that fact are the page bounds and the one
//! exception the single integer cannot express: the initial pass is anchored and
//! paged by offset, and an offset is legal only there (A164).

/// Rows per pull page when the client does not ask.
pub const DEFAULT_LIMIT: i64 = 200;

/// The largest page a client may ask for. A bigger number is clamped rather than
/// refused: the cap exists to bound one response, not to punish a caller for asking.
pub const MAX_LIMIT: i64 = 1000;

/// A validated `GET /sync/pull` query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PullQuery {
    pub cursor: i64,
    pub offset: i64,
    pub limit: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CursorError {
    #[error("cursor cannot be negative")]
    NegativeCursor,
    #[error("offset cannot be negative")]
    NegativeOffset,
    #[error("limit must be between 1 and {MAX_LIMIT}")]
    LimitOutOfRange { limit: i64 },
    #[error("offset is only meaningful when cursor is 0")]
    OffsetWithCursor,
}

/// What a cursor asks for. The two arms are different queries, not two spellings of
/// one: `Initial` is a snapshot pass over a rank-ordered stream (A164), `Incremental`
/// is a walk up the seq clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorKind {
    Initial { offset: i64 },
    Incremental { after: i64 },
}

impl PullQuery {
    /// Parses the three optional query parameters, defaulting the page size and
    /// clamping it to [`MAX_LIMIT`].
    ///
    /// A `limit` of 0 or a negative is a caller bug rather than a request for a
    /// smaller page, so it is a 400; anything above the cap is the caller asking for
    /// too much and is clamped. `offset` is meaningful only inside an initial pass:
    /// accepting it beside a non-zero cursor would page a rank-ordered stream with a
    /// seq cursor and deliver the wrong rows, which is worse than a refusal.
    pub fn parse(
        cursor: Option<i64>,
        offset: Option<i64>,
        limit: Option<i64>,
    ) -> Result<Self, CursorError> {
        let cursor = cursor.unwrap_or(0);
        if cursor < 0 {
            return Err(CursorError::NegativeCursor);
        }
        let offset = offset.unwrap_or(0);
        if offset < 0 {
            return Err(CursorError::NegativeOffset);
        }
        if offset > 0 && cursor != 0 {
            return Err(CursorError::OffsetWithCursor);
        }
        let limit = match limit {
            None => DEFAULT_LIMIT,
            Some(limit) if limit < 1 => return Err(CursorError::LimitOutOfRange { limit }),
            Some(limit) => limit.min(MAX_LIMIT),
        };
        Ok(Self {
            cursor,
            offset,
            limit,
        })
    }

    /// Which of the two queries this cursor is.
    pub fn kind(&self) -> CursorKind {
        if self.cursor == 0 {
            CursorKind::Initial {
                offset: self.offset,
            }
        } else {
            CursorKind::Incremental { after: self.cursor }
        }
    }
}

/// Whether a cursor is older than the history this account still retains (A165).
///
/// `min_retained_seq` is the per-user watermark compaction writes; 0 means nothing has
/// ever been compacted. A cursor of 0 is never stale, so a full resync is always
/// available. The comparison cannot be against `min(seq)`: `seq` comes from a global
/// sequence, so a brand-new account's first change can be seq 500, and a device at
/// cursor 0 would be told to resync on its very first request.
pub fn is_stale(cursor: i64, min_retained_seq: i64) -> bool {
    cursor > 0 && cursor < min_retained_seq
}
