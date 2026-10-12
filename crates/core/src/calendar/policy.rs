//! The conflict decision, as a function.
//!
//! The roadmap's conflict table has a row for the calendar, and its wording is "Google wins for
//! non-app events". That sentence is a rule someone has to remember, so this module is what
//! turns it into a function with a test — and A234 is what makes the *rest* of it structural:
//! the consent asks for `calendar.app.created` (write only to calendars we made) plus
//! `calendar.events.readonly` (read everything else, change nothing). So the case this module
//! decides is the case the token already prevents; the function exists so the *local* half —
//! what we do with a remote change — is testable without Google.
//!
//! Pure: no clock, no database, no network. The caller reads the local row, asks the remote,
//! and calls [`resolve`].
//!
//! **Why not "latest write wins" on the timestamp.** A mirrored study block is edited on the
//! phone and pushed; Google's copy then carries an `updated` *after* our push, because Google
//! sets it when it accepts the write. Comparing timestamps would therefore hand the remote a
//! win it did not earn and revert the student's own edit — silently, on the next poll. Origin
//! is the discriminator, not time.

/// Who the local row belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// `events.source = 'app'`: the student's own row, mirrored to Google.
    App,
    /// `events.source = 'google'`: a row we adopted from the remote. Read-only locally.
    Google,
}

/// The local row's state, as the decision needs it.
///
/// `local_dirty` is "the local copy has an edit the remote has not seen". For an [`Origin::App`]
/// row that is the student's own change; for an [`Origin::Google`] row it cannot happen (the
/// client is refused by A198), and the variant is still accepted so the function is total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalState {
    pub exists: bool,
    pub local_dirty: bool,
}

impl LocalState {
    /// A row with no pending local edit.
    pub fn clean() -> Self {
        Self {
            exists: true,
            local_dirty: false,
        }
    }

    /// A row with a local edit the remote has not seen.
    pub fn dirty() -> Self {
        Self {
            exists: true,
            local_dirty: true,
        }
    }

    /// No local row at all: the remote has something we have never seen.
    pub fn absent() -> Self {
        Self {
            exists: false,
            local_dirty: false,
        }
    }
}

/// What the remote says about the same event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteState {
    pub changed: bool,
    /// A remote tombstone: Google reports the event as `cancelled`.
    pub deleted: bool,
}

impl RemoteState {
    pub fn unchanged() -> Self {
        Self {
            changed: false,
            deleted: false,
        }
    }

    pub fn changed() -> Self {
        Self {
            changed: true,
            deleted: false,
        }
    }

    pub fn deleted() -> Self {
        Self {
            changed: true,
            deleted: true,
        }
    }
}

/// Which side wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Winner {
    /// Keep the local row, and push it to the remote.
    Local,
    /// Take the remote's version, overwriting the local row.
    Remote,
    /// Nothing to do: the two agree, or neither side has anything to say.
    Neither,
}

/// The roadmap's conflict table, as a function (A234).
///
/// Four rules, and each is the one a plausible implementation gets wrong:
///
///  1. **An adopted (`Google`) row takes the remote's version, always.** The local copy is a
///     mirror; there is no local edit to lose, because A198 refuses a client the write.
///  2. **An app row keeps a local edit and pushes it.** The remote cannot have "the newer"
///     version of a row only we write to: our push is what set the remote's copy, and Google
///     stamps its own `updated` when it accepts it.
///  3. **A remote tombstone is honoured for a Google row and ignored for an app row.** Google
///     cannot delete what the scope does not let it write, so a `cancelled` on a row we
///     created is either our own push echoing back or a user action we cannot act on — and
///     acting on it would delete the student's study block, which is exactly the outcome the
///     scope pair exists to prevent.
///  4. **A remote row we have never seen is adopted.** That is the "their Google events appear
///     here" half of the phase's goal.
pub fn resolve(origin: Origin, local: LocalState, remote: RemoteState) -> Winner {
    match origin {
        // Rule 3's second half, and it has to be checked before rule 1: a tombstone on a
        // Google row is a real deletion, and the two cases have the same origin.
        Origin::Google if remote.deleted => Winner::Remote,
        Origin::Google => {
            if remote.changed {
                Winner::Remote
            } else {
                Winner::Neither
            }
        }
        // Rule 3's first half: a remote tombstone for a row we created is ignored and the row
        // is re-pushed, because Google cannot delete what it cannot write.
        Origin::App if remote.deleted => Winner::Local,
        Origin::App => {
            if local.local_dirty {
                Winner::Local
            } else if remote.changed && !local.exists {
                // The row is ours by origin but absent locally — a local delete that has not
                // been pushed yet. The tombstone wins (the roadmap's "delete vs concurrent
                // edit": tombstone wins unless the user explicitly restores), so neither side
                // is overwritten here and the local delete is pushed by the ordinary path.
                Winner::Neither
            } else {
                Winner::Neither
            }
        }
    }
}

/// The origin of a stored `events.source`.
///
/// `source = 'app'` is the student's; `google` and `ics` are both the calendar's. A value this
/// build does not know is treated as the calendar's, which is the safe direction: refusing to
/// overwrite an unknown row is a stale row, while overwriting an app row that was mislabelled
/// is a lost edit.
pub fn origin_of(source: &str) -> Origin {
    if source == "app" {
        Origin::App
    } else {
        Origin::Google
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four cases the roadmap's table names, each asserted on its own so a regression
    /// points at the rule it broke.
    #[test]
    fn google_wins_for_an_event_we_did_not_create_and_we_win_for_a_study_block() {
        // An adopted row takes the remote's version, whether or not the local copy looks
        // "dirty" — a local edit cannot exist on it (A198), so a flag saying otherwise is
        // stale and must not win.
        assert_eq!(
            resolve(Origin::Google, LocalState::clean(), RemoteState::changed()),
            Winner::Remote
        );
        assert_eq!(
            resolve(Origin::Google, LocalState::dirty(), RemoteState::changed()),
            Winner::Remote
        );

        // Our own mirrored row: the local edit is the student's and it is pushed.
        assert_eq!(
            resolve(Origin::App, LocalState::dirty(), RemoteState::changed()),
            Winner::Local
        );

        // A remote tombstone for a Google row is a real deletion; for an app row it is
        // ignored and the row is re-pushed, because Google cannot delete what it cannot write.
        assert_eq!(
            resolve(Origin::Google, LocalState::clean(), RemoteState::deleted()),
            Winner::Remote
        );
        assert_eq!(
            resolve(Origin::App, LocalState::clean(), RemoteState::deleted()),
            Winner::Local
        );

        // And the quiet case: nothing moved on either side.
        assert_eq!(
            resolve(
                Origin::Google,
                LocalState::clean(),
                RemoteState::unchanged()
            ),
            Winner::Neither
        );
        assert_eq!(
            resolve(Origin::App, LocalState::clean(), RemoteState::unchanged()),
            Winner::Neither
        );
    }

    /// The case that makes the function worth having rather than a comment: a mirrored row whose
    /// remote `updated` is *newer* than our push must still keep the local edit. Comparing
    /// timestamps would hand the remote a win it did not earn and silently revert the student's
    /// own change on the next poll — which is why origin, not time, is the discriminator.
    #[test]
    fn a_newer_remote_timestamp_does_not_beat_a_local_edit() {
        // The remote reports a change (it always does after our own push, because Google stamps
        // its own `updated` on accept) and the local row has an edit.
        assert_eq!(
            resolve(Origin::App, LocalState::dirty(), RemoteState::changed()),
            Winner::Local,
            "origin decides, not recency"
        );
    }

    /// An unknown `source` is treated as the calendar's, which is the safe direction: a stale
    /// row is recoverable, a lost edit is not.
    #[test]
    fn an_unknown_source_is_the_calendars() {
        assert_eq!(origin_of("app"), Origin::App);
        assert_eq!(origin_of("google"), Origin::Google);
        assert_eq!(origin_of("ics"), Origin::Google);
        assert_eq!(origin_of("something-new"), Origin::Google);
        assert_eq!(origin_of(""), Origin::Google);
    }

    /// A remote event we have never seen is adopted, which is the "their Google events appear
    /// here" half of the phase's goal. Asserted separately because it is the one case where
    /// `local.exists` is false and the answer is still `Remote`.
    #[test]
    fn a_remote_row_we_have_never_seen_is_adopted() {
        assert_eq!(
            resolve(Origin::Google, LocalState::absent(), RemoteState::changed()),
            Winner::Remote
        );
        // And a tombstone for a row we never had is nothing to do: there is no local row to
        // delete, and creating then deleting one would write two change-log entries for
        // nothing.
        assert_eq!(
            resolve(Origin::Google, LocalState::absent(), RemoteState::deleted()),
            Winner::Remote
        );
    }
}
