//! The rule that decides which copy of a thing wins, written once for everything that syncs.
//!
//! # Why a stamp rather than a timestamp
//!
//! Two machines that sync have no shared clock, and the one whose clock is wrong would win every
//! conflict for as long as it stayed wrong. So nothing here asks when something happened. Each copy
//! carries a *revision* that only goes up, and the higher one is the newer one by definition.
//!
//! # The three fields, in the order they are consulted
//!
//! 1. **`revision`** — every change bumps it, so a copy that has been edited beats one that has not.
//! 2. **`deleted`** — a tie goes to the deleted one. Deletion wins ties on purpose: the alternative
//!    is a thing you removed coming back, which is the failure people actually notice.
//! 3. **`tie_breaker`** — sixteen random bytes, *stored with the record* and rerolled on every
//!    change. Two machines that edited the same thing to the same revision have to agree on which
//!    edit survives, and the only way to agree without talking is to compare something both of them
//!    already hold.
//!
//! # Why this makes sync order-independent
//!
//! Both ends run this comparison over the same two records and reach the same answer without
//! negotiating. So it does not matter which machine starts the sync, nor how many times it runs:
//! merging is idempotent, and running it backwards gives the same result as running it forwards.
//!
//! # What a tombstone is for
//!
//! A record that is deleted stays, with `deleted` set and its revision bumped. Removing the row
//! outright would be indistinguishable from never having had it — and the other end, seeing a
//! record we lack, would helpfully give it back on the next sync. The tombstone is what says *I
//! know about this one, and it is gone.*

/// What every syncable record carries so that two copies of it can be compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub revision: u64,
    pub deleted: bool,
    pub tie_breaker: [u8; 16],
}

/// The highest revision a record may carry: 2^53, the largest integer a Lua number or a JSON reader
/// holds exactly. A stored revision above it is damage, and a change that would pass it is refused.
pub const MAX_REVISION: u64 = 1 << 53;

impl Stamp {
    /// The stamp a record is born with.
    pub fn first() -> Option<Stamp> {
        Some(Stamp {
            revision: 1,
            deleted: false,
            tie_breaker: roll()?,
        })
    }

    /// Mark a change: up one revision, and a fresh tie-breaker.
    ///
    /// **Rerolled rather than kept**, because the tie-breaker settles which of two *different* edits
    /// survives. Carrying the old one forward would let a machine that edited first keep winning
    /// every later tie against the same record.
    ///
    /// `None`, with the stamp unchanged, when the revision is already [`MAX_REVISION`] or the
    /// system has no randomness to give.
    pub fn advance(&mut self) -> Option<()> {
        let next = self
            .revision
            .checked_add(1)
            .filter(|next| *next <= MAX_REVISION)?;
        self.tie_breaker = roll()?;
        self.revision = next;
        Some(())
    }

    /// The same, and the record is gone.
    pub fn bury(&mut self) -> Option<()> {
        self.advance()?;
        self.deleted = true;
        Some(())
    }

    /// Whether this copy beats `other`. Equal stamps are the same copy, and neither wins.
    pub fn wins_over(&self, other: &Stamp) -> bool {
        self.order() > other.order()
    }

    /// The three fields in the order they decide, as one comparable value.
    fn order(&self) -> (u64, bool, [u8; 16]) {
        (self.revision, self.deleted, self.tie_breaker)
    }

    /// Whether this stamp says anything at all — a revision of zero is not a record.
    pub fn is_real(&self) -> bool {
        self.revision != 0
    }
}

/// Sixteen bytes nobody can predict.
fn roll() -> Option<[u8; 16]> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).ok()?;
    Some(bytes)
}

/// Which of two copies survives, and whether that means a change on either side.
///
/// The one function every store's merge is written in terms of, so that history, macros and secrets
/// cannot drift into three different ideas of what "newer" means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// They already agree. Nothing to do on either side.
    Agreed,
    /// Ours wins: send it over there.
    Ours,
    /// Theirs wins: take it.
    Theirs,
}

/// Compare what we have against what they have.
///
/// `None` on either side means *that machine has never heard of this record*, which is different
/// from having a tombstone for it — the whole reason tombstones exist.
pub fn settle(ours: Option<&Stamp>, theirs: Option<&Stamp>) -> Verdict {
    match (ours, theirs) {
        (None, None) => Verdict::Agreed,
        (Some(_), None) => Verdict::Ours,
        (None, Some(_)) => Verdict::Theirs,
        (Some(ours), Some(theirs)) => {
            if ours == theirs {
                Verdict::Agreed
            } else if theirs.wins_over(ours) {
                Verdict::Theirs
            } else {
                Verdict::Ours
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(revision: u64, deleted: bool, tie: u8) -> Stamp {
        Stamp {
            revision,
            deleted,
            tie_breaker: [tie; 16],
        }
    }

    #[test]
    fn a_higher_revision_wins() {
        assert!(at(2, false, 1).wins_over(&at(1, false, 9)));
        assert!(!at(1, false, 9).wins_over(&at(2, false, 1)));
    }

    /// **Deletion wins ties**, or a thing you removed comes back on the next sync.
    #[test]
    fn a_tie_goes_to_the_deleted_one() {
        assert!(at(3, true, 1).wins_over(&at(3, false, 9)));
        assert!(!at(3, false, 9).wins_over(&at(3, true, 1)));
    }

    /// And a tie there is settled by something both machines already hold, so both reach the same
    /// answer without asking each other.
    #[test]
    fn a_tie_there_is_settled_the_same_way_on_both_ends() {
        let one = at(3, false, 7);
        let other = at(3, false, 4);
        assert!(one.wins_over(&other));
        assert!(!other.wins_over(&one));
        // Whichever end asks, the same record survives.
        assert_eq!(settle(Some(&one), Some(&other)), Verdict::Ours);
        assert_eq!(settle(Some(&other), Some(&one)), Verdict::Theirs);
    }

    /// Identical copies are not a conflict, and syncing again must move nothing.
    #[test]
    fn the_same_stamp_is_agreement() {
        let same = at(4, false, 2);
        assert_eq!(settle(Some(&same), Some(&same)), Verdict::Agreed);
        assert!(!same.wins_over(&same));
        assert_eq!(settle(None, None), Verdict::Agreed);
    }

    /// A record one machine has never seen travels; a tombstone is not the same as never having it.
    #[test]
    fn never_heard_of_it_is_not_the_same_as_deleted() {
        let alive = at(1, false, 1);
        let buried = at(2, true, 1);
        assert_eq!(settle(Some(&alive), None), Verdict::Ours);
        assert_eq!(settle(None, Some(&alive)), Verdict::Theirs);
        // The tombstone beats the copy that never heard about the deletion.
        assert_eq!(settle(Some(&alive), Some(&buried)), Verdict::Theirs);
    }

    #[test]
    fn a_change_moves_the_stamp_and_a_burial_ends_it() {
        let mut stamp = Stamp::first().expect("randomness");
        assert_eq!(stamp.revision, 1);
        assert!(!stamp.deleted);
        assert!(stamp.is_real());

        let born = stamp;
        stamp.advance().expect("randomness");
        assert!(stamp.wins_over(&born));
        assert_eq!(stamp.revision, 2);
        // Rerolled, or the machine that edited first would win every later tie.
        assert_ne!(stamp.tie_breaker, born.tie_breaker);

        let edited = stamp;
        stamp.bury().expect("randomness");
        assert!(stamp.deleted);
        assert!(stamp.wins_over(&edited));
    }

    /// A revision cannot be pushed past the bound, and a refused change leaves the stamp as it was.
    #[test]
    fn a_revision_stops_at_the_bound() {
        let mut last = at(MAX_REVISION - 1, false, 5);
        last.advance().expect("one more");
        assert_eq!(last.revision, MAX_REVISION);

        let pinned = last;
        assert!(last.advance().is_none());
        assert!(last.bury().is_none());
        assert_eq!(last, pinned);

        let mut absurd = at(u64::MAX, false, 0);
        assert!(absurd.advance().is_none());
        assert_eq!(absurd.revision, u64::MAX);
    }
}
