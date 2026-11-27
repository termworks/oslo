//! Variables whose value is read at the moment they are expanded.
//!
//! `$EPOCHREALTIME` is not a variable anybody set; it is the clock, spelled as a variable. So it
//! cannot live in the variable table — it has to be computed on each expansion, which is what this
//! module is for.
//!
//! # An assignment
//!
//! `SECONDS=0` re-bases the count and `RANDOM=42` seeds the generator. An assignment to the pure
//! clocks (`EPOCHSECONDS`, `EPOCHREALTIME`, `BASH_MONOSECONDS`) and to `SRANDOM` is discarded, as
//! bash discards it. `unset` turns any of these names back into an ordinary variable.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// When the shell started, for `$SECONDS`.
static STARTED: AtomicU64 = AtomicU64::new(0);

/// Where `$SECONDS` counts from: zero, or wherever an assignment last put it.
static ORIGIN: AtomicI64 = AtomicI64::new(0);

/// Whether `$SECONDS` has been read, after which bash gives it the integer attribute.
static SECONDS_READ: AtomicBool = AtomicBool::new(false);

/// The generator's state, bash's `rseed`.
static SEED: AtomicU32 = AtomicU32::new(1);

/// The last number `$RANDOM` answered, which the next draw may not repeat.
static LAST: AtomicU32 = AtomicU32::new(0);

/// The process that owns [`SEED`]; any other process reseeds before drawing.
static SEED_PID: AtomicU32 = AtomicU32::new(0);

/// The names an `unset` has turned back into ordinary variables, as a bitmask over [`ALL`].
static RETIRED: AtomicU64 = AtomicU64::new(0);

/// Every special name whose `unset` this module records, in the order [`RETIRED`]'s bits are in.
const ALL: [&str; 12] = [
    "EPOCHREALTIME",
    "EPOCHSECONDS",
    "SECONDS",
    "RANDOM",
    "SRANDOM",
    "BASH_MONOSECONDS",
    "BASHPID",
    "BASH_ARGV0",
    "HISTCMD",
    "GROUPS",
    "DIRSTACK",
    "BASH_SUBSHELL",
];

/// Record the start time and seed the generator. Called once, from `Environment::new`.
pub fn start() {
    STARTED.store(now_secs(), Ordering::Relaxed);
    reseed();
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

/// Seconds on the monotonic clock, bash's `$BASH_MONOSECONDS`.
fn monotonic_secs() -> i64 {
    let mut now = nix::libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `now` is a valid timespec this call owns.
    match unsafe { nix::libc::clock_gettime(nix::libc::CLOCK_MONOTONIC, &mut now) } {
        0 => now.tv_sec,
        _ => 0,
    }
}

/// The value of `name` if it is one of these, or `None`.
pub fn value(name: &str) -> Option<String> {
    if retired(name) {
        return None;
    }
    Some(match name {
        "EPOCHREALTIME" => {
            let d = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
            format!("{}.{:06}", d.as_secs(), d.subsec_micros())
        }
        "EPOCHSECONDS" => now_secs().to_string(),
        "BASH_MONOSECONDS" => monotonic_secs().to_string(),
        "SECONDS" => {
            SECONDS_READ.store(true, Ordering::Relaxed);
            let elapsed = now_secs().saturating_sub(STARTED.load(Ordering::Relaxed));
            (ORIGIN.load(Ordering::Relaxed) + elapsed as i64).to_string()
        }
        "RANDOM" => next_random().to_string(),
        "SRANDOM" => match getrandom_u32() {
            Some(n) => n.to_string(),
            None => intrand32(SEED.load(Ordering::Relaxed)).to_string(),
        },
        _ => return None,
    })
}

/// Whether `name` is one of the clock or generator names, without computing it.
pub fn is_dynamic(name: &str) -> bool {
    matches!(
        name,
        "EPOCHREALTIME" | "EPOCHSECONDS" | "SECONDS" | "RANDOM" | "SRANDOM" | "BASH_MONOSECONDS"
    ) && !retired(name)
}

/// Whether `name` carries bash's integer attribute while it is still special.
pub fn is_integer(name: &str) -> bool {
    let integer = match name {
        "RANDOM" | "SRANDOM" | "BASHPID" | "HISTCMD" => true,
        "SECONDS" => SECONDS_READ.load(Ordering::Relaxed),
        _ => false,
    };
    integer && !retired(name)
}

fn bit_of(name: &str) -> Option<u64> {
    ALL.iter().position(|n| *n == name).map(|at| 1 << at)
}

/// Whether `unset` has turned `name` into an ordinary variable.
pub fn retired(name: &str) -> bool {
    bit_of(name).is_some_and(|bit| RETIRED.load(Ordering::Relaxed) & bit != 0)
}

/// Take an assignment to one of these names. Answers whether it was taken.
///
/// `eval` evaluates the text as arithmetic, which is how bash reads a value assigned to an
/// integer variable; `None` means the expression failed.
pub fn assign(name: &str, value: &str, eval: &mut dyn FnMut(&str) -> Option<i64>) -> bool {
    if retired(name) {
        return false;
    }
    match name {
        "SECONDS" => {
            let number = match SECONDS_READ.load(Ordering::Relaxed) {
                true => eval(value),
                false => value.trim().parse::<i64>().ok(),
            };
            STARTED.store(now_secs(), Ordering::Relaxed);
            ORIGIN.store(number.unwrap_or(0), Ordering::Relaxed);
            true
        }
        "RANDOM" => {
            if let Some(seed) = eval(value) {
                SEED.store(seed as u32, Ordering::Relaxed);
                LAST.store(0, Ordering::Relaxed);
                SEED_PID.store(std::process::id(), Ordering::Relaxed);
            }
            true
        }
        "SRANDOM" | "EPOCHSECONDS" | "EPOCHREALTIME" | "BASH_MONOSECONDS" => true,
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

/// Seed the generator for this process, from the clock, the pid and the kernel.
fn reseed() {
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() ^ d.as_secs() as u32)
        .unwrap_or(0);
    let seed = getrandom_u32().unwrap_or(0) ^ nanos ^ pid;
    SEED.store(seed, Ordering::Relaxed);
    LAST.store(0, Ordering::Relaxed);
    SEED_PID.store(pid, Ordering::Relaxed);
}

/// Park and Miller's minimal standard generator, computed the way bash's `intrand32` does.
fn intrand32(last: u32) -> u32 {
    let ret: u32 = if last == 0 { 123_459_876 } else { last };
    let h = (ret / 127_773) as i32;
    let l = ret.wrapping_sub(127_773u32.wrapping_mul(h as u32)) as i32;
    let t = 16_807i32
        .wrapping_mul(l)
        .wrapping_sub(2_836i32.wrapping_mul(h));
    if t < 0 {
        t.wrapping_add(0x7fff_ffff) as u32
    } else {
        t as u32
    }
}

/// One step of bash's `brand`: the next state, and the 15-bit number drawn from it.
fn step(seed: u32) -> (u32, u32) {
    let next = intrand32(seed);
    (next, ((next >> 16) ^ (next & 0xffff)) & 0x7fff)
}

/// The next `$RANDOM`, reseeding first in a forked child and never repeating the last answer.
fn next_random() -> u32 {
    if SEED_PID.load(Ordering::Relaxed) != std::process::id() {
        reseed();
    }
    let last = LAST.load(Ordering::Relaxed);
    let mut seed = SEED.load(Ordering::Relaxed);
    let drawn = loop {
        let (next, drawn) = step(seed);
        seed = next;
        if drawn != last {
            break drawn;
        }
    };
    SEED.store(seed, Ordering::Relaxed);
    LAST.store(drawn, Ordering::Relaxed);
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

/// These tests walk process-wide statics, so they take turns.
#[cfg(test)]
fn serialised() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number(text: &str) -> Option<i64> {
        text.trim().parse().ok()
    }

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
        let mono: i64 = value("BASH_MONOSECONDS")
            .expect("set")
            .parse()
            .expect("number");
        assert!(mono >= 0);
    }

    #[test]
    fn the_clock_moves() {
        let _serial = serialised();
        start();
        let first = value("EPOCHREALTIME").expect("set");
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = value("EPOCHREALTIME").expect("set");
        assert_ne!(first, second);
    }

    #[test]
    fn random_is_a_sequence_in_bashs_range() {
        let _serial = serialised();
        start();
        let draws: Vec<u64> = (0..64)
            .map(|_| value("RANDOM").expect("set").parse().expect("number"))
            .collect();
        assert!(draws.iter().all(|&n| n < 32768), "outside 0..32767");
        assert!(
            draws.windows(2).all(|w| w[0] != w[1]),
            "a repeat: {draws:?}"
        );
    }

    #[test]
    fn srandom_is_wider() {
        let _serial = serialised();
        let n: u64 = value("SRANDOM").expect("set").parse().expect("number");
        assert!(n <= u64::from(u32::MAX));
    }

    #[test]
    fn nothing_else_is_dynamic() {
        assert!(is_dynamic("EPOCHREALTIME"));
        assert!(is_dynamic("RANDOM"));
        assert!(!is_dynamic("PATH"));
        assert!(!is_dynamic("epochrealtime"), "names are case-sensitive");
        assert_eq!(value("PATH"), None);
    }

    /// The numbers bash 5.3 prints for `RANDOM=42`, `RANDOM=0` and `RANDOM=-1`.
    #[test]
    fn the_generator_is_bashs() {
        let draw = |seed: u32, n: usize| {
            let mut at = seed;
            (0..n)
                .map(|_| {
                    let (next, drawn) = step(at);
                    at = next;
                    drawn
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(draw(42, 3), [17772, 26794, 1435]);
        assert_eq!(draw(0, 2), [20814, 24386]);
        assert_eq!(draw(u32::MAX, 1), [16807]);
    }

    #[test]
    fn assigning_random_seeds_the_sequence() {
        let _serial = serialised();
        RETIRED.store(0, Ordering::Relaxed);
        assert!(assign("RANDOM", "42", &mut number));
        let drawn: Vec<String> = (0..3).map(|_| value("RANDOM").expect("set")).collect();
        assert_eq!(drawn, ["17772", "26794", "1435"]);
    }

    #[test]
    fn seconds_counts_from_where_it_was_set() {
        let _serial = serialised();
        RETIRED.store(0, Ordering::Relaxed);
        start();
        assert!(assign("SECONDS", "100", &mut number));
        assert_eq!(value("SECONDS").as_deref(), Some("100"));
        STARTED.store(now_secs() - 5, Ordering::Relaxed);
        assert_eq!(value("SECONDS").as_deref(), Some("105"));
        assert!(assign("SECONDS", "abc", &mut number));
        assert_eq!(value("SECONDS").as_deref(), Some("0"));
    }

    #[test]
    fn unsetting_retires_the_name() {
        let _serial = serialised();
        RETIRED.store(0, Ordering::Relaxed);
        assert!(value("SECONDS").is_some(), "it starts as the clock");
        retire("SECONDS");
        assert_eq!(value("SECONDS"), None, "and is nothing afterwards");
        assert!(!is_dynamic("SECONDS"));
        assert!(!assign("SECONDS", "5", &mut number));
        assert!(value("RANDOM").is_some(), "only the name that was unset");
        RETIRED.store(0, Ordering::Relaxed);
    }
}
