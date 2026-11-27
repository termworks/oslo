//! Variables whose value is read at the moment they are expanded.
//!
//! `$EPOCHREALTIME` is not a variable anybody set; it is the clock, spelled as a variable. So it
//! cannot live in the variable table — it has to be computed on each expansion, which is what this
//! module is for.
//!
//! # Why a shell needs these at all
//!
//! Because every prompt tool measures how long your last command took, and the only portable way
//! to do that is to read a high-resolution clock twice: once in preexec, once in precmd. Without
//! `$EPOCHREALTIME`, starship shells out to `starship time`, oh-my-posh to `oh-my-posh get millis`
//! and hexe to `date +%s%3N` — **a process fork per command, on the path between pressing Enter and
//! seeing a prompt.** That is the whole cost of not having a two-line feature.
//!
//! `$SECONDS` is deliberately here too, and deliberately useless for that purpose: bash specifies
//! it at one-second resolution, so it measures nothing a person would notice. It exists because
//! scripts use it for coarse timeouts.
//!
//! # An assignment wins
//!
//! Every name here is overridable. `SECONDS=0` is an idiom — it resets the count — and a script
//! that says `RANDOM=42` is asking for a reproducible sequence. So the table is consulted only
//! when the variable has not been set, which also means none of these can shadow something a
//! parent process exported.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// When the shell started, for `$SECONDS`.
static STARTED: AtomicU64 = AtomicU64::new(0);

/// The seed `$RANDOM` walks, so a sequence is a sequence rather than the same number twice.
static SEED: AtomicU64 = AtomicU64::new(0);

/// Record the start time and seed the generator. Called once, from `Environment::new`.
pub fn start() {
    STARTED.store(now_secs(), Ordering::Relaxed);
    // Seeded from the clock and the pid, so two shells started in the same second still differ.
    let seed = now_nanos() ^ (u64::from(std::process::id()) << 32);
    SEED.store(usable_seed(seed), Ordering::Relaxed);
}

/// The second the shell started, as an epoch time.
pub fn started() -> u64 {
    STARTED.load(Ordering::Relaxed)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// The value of `name` if it is one of these, or `None`.
pub fn value(name: &str) -> Option<String> {
    if retired(name) {
        return None;
    }
    Some(match name {
        // Six decimal places, and **always a `.`** — bash's is locale-dependent, which is why
        // oh-my-posh strips every non-digit before parsing it. A prompt tool reading this one can
        // just split on the dot.
        "EPOCHREALTIME" => {
            let d = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
            format!("{}.{:06}", d.as_secs(), d.subsec_micros())
        }
        "EPOCHSECONDS" => now_secs().to_string(),
        // Whole seconds since the shell started, as bash specifies it. Useless for timing a
        // command and fine for "have we been waiting five minutes".
        "SECONDS" => now_secs()
            .saturating_sub(STARTED.load(Ordering::Relaxed))
            .saturating_add(ORIGIN.load(Ordering::Relaxed))
            .to_string(),
        // 0..32767, bash's range. xorshift64* rather than anything from a crate: this is a shell
        // variable for picking a temp-file suffix, not a source of randomness anybody should be
        // trusting, and saying so in the code is better than implying otherwise with a dependency.
        "RANDOM" => (next_random() % 32768).to_string(),
        // 32 bits from the OS, for the cases `$RANDOM` is too weak and too narrow for. bash reads
        // `getrandom`; falling back to the same generator is honest about what it is.
        "SRANDOM" => match getrandom_u32() {
            Some(n) => n.to_string(),
            None => (next_random() as u32).to_string(),
        },
        _ => return None,
    })
}

/// Whether `name` is one of ours, without computing it.
pub fn is_dynamic(name: &str) -> bool {
    matches!(
        name,
        "EPOCHREALTIME" | "EPOCHSECONDS" | "SECONDS" | "RANDOM" | "SRANDOM"
    ) && !retired(name)
}

/// Where `$SECONDS` counts from: the shell's start, or wherever an assignment last put it.
static ORIGIN: AtomicU64 = AtomicU64::new(0);

/// The names an `unset` has turned back into ordinary variables, as a bitmask over [`ALL`].
static RETIRED: AtomicU64 = AtomicU64::new(0);

/// Every name this module answers for, in the order [`RETIRED`]'s bits are in.
const ALL: [&str; 5] = [
    "EPOCHREALTIME",
    "EPOCHSECONDS",
    "SECONDS",
    "RANDOM",
    "SRANDOM",
];

fn bit_of(name: &str) -> Option<u64> {
    ALL.iter().position(|n| *n == name).map(|at| 1 << at)
}

fn retired(name: &str) -> bool {
    bit_of(name).is_some_and(|bit| RETIRED.load(Ordering::Relaxed) & bit != 0)
}

/// Take an assignment to one of these names, if it is one this module should answer rather than
/// store. Answers whether it did.
///
/// **`SECONDS=0` resets the count; it does not freeze it.** The module note above says exactly
/// that, and the assignment used to be stored as an ordinary string that shadowed the generator for
/// good — so `SECONDS=0` pinned it at zero for the life of the shell, and `RANDOM=n` made every
/// later `$RANDOM` answer `n`. bash re-bases the one and seeds the other, and a script saying
/// `RANDOM=42` is asking for a reproducible *sequence*.
///
/// **A value that is not a number is zero**, which is what bash does: `SECONDS=abc` counts from 0
/// and `RANDOM=abc` seeds as `RANDOM=0` would. Both were checked rather than assumed, and so was
/// `SECONDS=2+3` — bash answers 0 there too, so this is a plain number or nothing, not arithmetic.
pub fn assign(name: &str, value: &str) -> bool {
    if retired(name) {
        return false;
    }
    let number = || value.trim().parse::<u64>().unwrap_or(0);
    match name {
        "SECONDS" => {
            STARTED.store(now_secs(), Ordering::Relaxed);
            ORIGIN.store(number(), Ordering::Relaxed);
            true
        }
        // Seeded rather than stored: the sequence that follows is reproducible, which is the whole
        // reason a script writes this.
        "RANDOM" => {
            SEED.store(usable_seed(number()), Ordering::Relaxed);
            true
        }
        _ => false,
    }
}

/// `unset SECONDS` makes it an ordinary variable, as in bash — after which it is empty rather than
/// the clock, and stays that way for the life of the shell.
pub fn retire(name: &str) {
    if let Some(bit) = bit_of(name) {
        RETIRED.fetch_or(bit, Ordering::Relaxed);
    }
}

/// A seed xorshift64 can actually walk.
///
/// **Not `n | 1`**, which was the guard against a zero seed and quietly folded every even seed onto
/// its odd neighbour: `RANDOM=42` and `RANDOM=43` both became 43 and produced the same sequence.
/// Zero is the only value the generator cannot use — it stays zero for ever — so zero is the only
/// one worth replacing.
fn usable_seed(n: u64) -> u64 {
    match n {
        0 => 0x9E37_79B9_7F4A_7C15,
        n => n,
    }
}

/// xorshift64*: the next state, and the number drawn from it.
///
/// **Pure, so that "the same seed gives the same sequence" can be asserted at all.** `SEED` is a
/// process-global and `Environment::new` re-seeds it — which is right for a shell, where there is
/// one, and impossible to test around in a binary that builds a thousand of them on as many
/// threads. The property belongs to the arithmetic; the global is only where the state is kept.
fn step(seed: u64) -> (u64, u64) {
    let mut x = seed;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    (x, x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33)
}

/// The next number, walking the process's seed.
fn next_random() -> u64 {
    let (next, drawn) = step(SEED.load(Ordering::Relaxed));
    SEED.store(next, Ordering::Relaxed);
    drawn
}

/// Four bytes from the kernel, or `None` where that is not available.
fn getrandom_u32() -> Option<u32> {
    let mut bytes = [0u8; 4];
    // SAFETY: a four-byte buffer this call owns, and `getrandom` writes at most that many.
    let n = unsafe {
        nix::libc::getrandom(
            bytes.as_mut_ptr().cast(),
            bytes.len(),
            nix::libc::GRND_NONBLOCK,
        )
    };
    (n == bytes.len() as isize).then(|| u32::from_ne_bytes(bytes))
}

/// These tests walk process-wide statics — the clock's origin, the generator's state, the retired
/// bitmask — so they take turns.
///
/// **Both test modules in this file share it**, which is the whole point of it living out here.
/// Two mutexes is no exclusion at all: `random_is_a_sequence_in_bashs_range` draws sixty-four
/// numbers from the same generator `random_is_seeded_into_a_reproducible_sequence` had just
/// seeded, so the seeded sequence came back different about one run in ten.
#[cfg(test)]
fn serialised() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_variables_are_the_clock() {
        let _serial = serialised();
        start();
        let real = value("EPOCHREALTIME").expect("set");
        let (secs, micros) = real.split_once('.').expect("always a dot, never a comma");
        assert!(secs.parse::<u64>().expect("seconds") > 1_600_000_000);
        assert_eq!(micros.len(), 6, "six places, always: {real:?}");
        assert!(micros.chars().all(|c| c.is_ascii_digit()));

        let whole: u64 = value("EPOCHSECONDS").expect("set").parse().expect("number");
        assert_eq!(whole.to_string(), secs, "the two must agree");
    }

    /// It has to actually advance, or a duration computed from it is always zero.
    #[test]
    fn the_clock_moves() {
        let _serial = serialised();
        start();
        let first = value("EPOCHREALTIME").expect("set");
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = value("EPOCHREALTIME").expect("set");
        assert_ne!(first, second, "a duration measured with this would be 0");
    }

    #[test]
    fn seconds_starts_at_zero() {
        let _serial = serialised();
        start();
        assert_eq!(value("SECONDS").as_deref(), Some("0"));
    }

    /// A sequence, not the same number repeatedly, and inside bash's range.
    #[test]
    fn random_is_a_sequence_in_bashs_range() {
        let _serial = serialised();
        start();
        let draws: Vec<u64> = (0..64)
            .map(|_| value("RANDOM").expect("set").parse().expect("number"))
            .collect();
        assert!(draws.iter().all(|&n| n < 32768), "outside 0..32767");
        assert!(
            draws.windows(2).any(|w| w[0] != w[1]),
            "the same number every time: {draws:?}"
        );
    }

    #[test]
    fn srandom_is_wider() {
        let _serial = serialised();
        start();
        let n: u64 = value("SRANDOM").expect("set").parse().expect("number");
        assert!(n <= u64::from(u32::MAX));
    }

    #[test]
    fn nothing_else_is_dynamic() {
        let _serial = serialised();
        assert!(is_dynamic("EPOCHREALTIME"));
        assert!(is_dynamic("RANDOM"));
        assert!(!is_dynamic("PATH"));
        assert!(!is_dynamic("epochrealtime"), "names are case-sensitive");
        assert_eq!(value("PATH"), None);
    }
}

/// **An assignment re-bases the clock and seeds the generator; it does not replace them.**
///
/// The module note above has always said so. The assignment was stored as an ordinary string that
/// shadowed the value for good, so `SECONDS=0` pinned it at zero for the life of the shell and
/// `RANDOM=42` made every later `$RANDOM` answer 42 — which is exactly what a script writing either
/// of those is trying not to get.
#[cfg(test)]
mod assignment_tests {
    use super::*;

    /// Put the statics back so a test does not decide the next one's answer.
    fn fresh() {
        RETIRED.store(0, Ordering::Relaxed);
        ORIGIN.store(0, Ordering::Relaxed);
        start();
    }

    #[test]
    fn seconds_counts_from_where_it_was_set() {
        let _serial = serialised();
        fresh();

        assert!(assign("SECONDS", "0"));
        assert_eq!(value("SECONDS").as_deref(), Some("0"));

        // Counting from a number counts *up* from it, rather than reporting it forever.
        assert!(assign("SECONDS", "100"));
        assert_eq!(value("SECONDS").as_deref(), Some("100"));
        ORIGIN.store(100, Ordering::Relaxed);
        STARTED.store(now_secs() - 5, Ordering::Relaxed);
        assert_eq!(
            value("SECONDS").as_deref(),
            Some("105"),
            "five seconds after being set to a hundred"
        );

        // Not a number is zero, which is bash's answer for `SECONDS=abc` and for `SECONDS=2+3`.
        assert!(assign("SECONDS", "abc"));
        assert_eq!(value("SECONDS").as_deref(), Some("0"));
        assert!(assign("SECONDS", "2+3"));
        assert_eq!(value("SECONDS").as_deref(), Some("0"));
        fresh();
    }

    /// Four numbers from `seed`, taken from the arithmetic rather than from the process's own
    /// state — see [`step`] for why the state cannot be held still long enough to ask it twice.
    fn sequence_from(seed: u64) -> Vec<u64> {
        let mut at = usable_seed(seed);
        (0..4)
            .map(|_| {
                let (next, drawn) = step(at);
                at = next;
                drawn % 32768
            })
            .collect()
    }

    /// **The same seed gives the same sequence, and a different one does not.**
    ///
    /// `RANDOM=42` asks for a reproducible sequence; storing the assignment instead made every
    /// later `$RANDOM` answer 42, and seeding with `n | 1` made 42 and 43 the same sequence.
    #[test]
    fn random_is_seeded_into_a_reproducible_sequence() {
        assert_eq!(sequence_from(42), sequence_from(42));
        assert_ne!(sequence_from(42), sequence_from(43));
        // A sequence, not one number repeated.
        assert!(
            sequence_from(42)
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                > 1
        );
        // And never the seed itself, which is what storing the assignment produced.
        assert!(!sequence_from(42).contains(&42));
    }

    /// The wiring: an assignment seeds the generator rather than being stored as the value.
    ///
    /// Asserted on one draw, not on a sequence — `Environment::new` re-seeds the process's state
    /// and a thousand of them are built on as many threads while this runs.
    #[test]
    fn assigning_random_seeds_rather_than_stores() {
        let _serial = serialised();
        fresh();
        assign("RANDOM", "42");
        let drawn = value("RANDOM").expect("set");
        assert_ne!(drawn, "42", "the seed is not the answer");
        assert!(
            drawn.parse::<u32>().expect("a number") < 32768,
            "in bash's range"
        );
        fresh();
    }

    /// `unset SECONDS` makes it an ordinary variable, as in bash — after which it is empty rather
    /// than the clock, and an assignment to it is a plain assignment again.
    #[test]
    fn unsetting_retires_the_name() {
        let _serial = serialised();
        fresh();

        assert!(value("SECONDS").is_some(), "it starts as the clock");
        retire("SECONDS");
        assert_eq!(value("SECONDS"), None, "and is nothing afterwards");
        assert!(!is_dynamic("SECONDS"), "so the store keeps it instead");
        assert!(
            !assign("SECONDS", "5"),
            "and an assignment is an ordinary one"
        );

        // Only the name that was unset.
        assert!(value("RANDOM").is_some());
        fresh();
    }
}
