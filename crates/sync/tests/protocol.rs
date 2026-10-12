//! The cursor rules, the page bounds and the payload allowlist: the properties the
//! server's pull and push routes are built on, asserted against the wire crate so a
//! change to either end fails here.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use stvdy_sync::proto::cursor::{
    CursorError, CursorKind, DEFAULT_LIMIT, MAX_LIMIT, PullQuery, is_stale,
};
use stvdy_sync::proto::{Entity, PayloadShape};

/// The default and the cap are the contract's numbers, and a request outside them is
/// clamped rather than refused — except a limit of 0 or a negative, which is a caller
/// bug and not a smaller page.
#[test]
fn the_page_size_is_defaulted_and_capped() {
    assert_eq!(
        PullQuery::parse(None, None, None).unwrap().limit,
        DEFAULT_LIMIT
    );
    assert_eq!(
        PullQuery::parse(Some(0), None, Some(10_000)).unwrap().limit,
        MAX_LIMIT
    );
    assert_eq!(PullQuery::parse(Some(0), None, Some(1)).unwrap().limit, 1);
    assert!(matches!(
        PullQuery::parse(Some(0), None, Some(0)),
        Err(CursorError::LimitOutOfRange { .. })
    ));
    assert!(matches!(
        PullQuery::parse(Some(-1), None, None),
        Err(CursorError::NegativeCursor)
    ));
}

/// `offset` pages the initial pass, and it may travel with the pass's anchor so the
/// pass set cannot change underneath the client (A164). A bare non-zero cursor is the
/// incremental walk.
#[test]
fn an_offset_pages_the_initial_pass_and_a_bare_cursor_walks_the_clock() {
    assert!(matches!(
        PullQuery::parse(Some(0), Some(20), None).unwrap().kind(),
        CursorKind::Initial { offset: 20 }
    ));
    assert!(matches!(
        PullQuery::parse(Some(41), None, None).unwrap().kind(),
        CursorKind::Incremental { after: 41 }
    ));
    // A cursor with an offset is a later page of the anchored pass, not an
    // incremental request: the cursor is the anchor the pass began at.
    let anchored = PullQuery::parse(Some(41), Some(20), None).unwrap();
    assert!(matches!(
        anchored.kind(),
        CursorKind::Initial { offset: 20 }
    ));
    assert_eq!(anchored.anchor(), Some(41));
    // The first page of a pass carries no anchor: the server derives it from the
    // account's current max(seq).
    assert_eq!(
        PullQuery::parse(Some(0), None, None).unwrap().anchor(),
        None
    );
}

/// A new device must never be told to resync: `seq` comes from a global sequence, so an
/// account's first change can be seq 500 and a watermark comparison against `min(seq)`
/// would 410 the very first request (A165).
#[test]
fn a_cursor_of_zero_is_never_stale_and_a_cursor_below_the_watermark_is() {
    assert!(!is_stale(0, 500));
    assert!(!is_stale(0, 0));
    assert!(!is_stale(500, 500));
    assert!(is_stale(499, 500));
}

/// The payload allowlist is the leak surface: `user_id` is the caller's own and
/// redundant, and the two storage keys are internal (A168).
#[test]
fn the_payload_allowlist_excludes_the_identity_and_the_storage_keys() {
    for entity in Entity::ALL {
        let columns = entity.payload_columns();
        assert!(!columns.is_empty(), "{entity:?} ships no columns at all");
        assert!(
            !columns.contains(&"user_id"),
            "{entity:?} ships user_id; every row already belongs to the caller"
        );
        assert!(
            columns.contains(&"updated_at"),
            "{entity:?} must ship updated_at: the client's LWW display needs it"
        );
    }

    let source = Entity::Source.payload_columns();
    assert!(
        !source.contains(&"file_key"),
        "an object key is not a client field"
    );
    assert!(
        !source.contains(&"preview_key"),
        "an object key is not a client field"
    );
    assert!(
        source.contains(&"status"),
        "the client renders ingestion status"
    );

    assert_eq!(Entity::Task.payload_shape(), PayloadShape::PatchWithBase);
    assert_eq!(Entity::Event.payload_shape(), PayloadShape::PatchWithBase);
    assert_eq!(Entity::FlashcardReview.payload_shape(), PayloadShape::Row);
    assert_eq!(Entity::UserSettings.payload_shape(), PayloadShape::Row);
}
