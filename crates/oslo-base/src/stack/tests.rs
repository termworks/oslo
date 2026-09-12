use super::{RESERVE, exhausted, headroom, mark};

/// An unmarked thread answers "unknown", and unknown is never a refusal.
///
/// This is the safety property of the whole module: a wrong refusal would stop scripts that are
/// nowhere near the limit, which is worse than the crash it guards against.
#[test]
fn an_unmarked_thread_refuses_nothing() {
    std::thread::spawn(|| {
        assert_eq!(headroom(), None, "nothing marked this thread");
        assert!(!exhausted(), "unknown must not read as exhausted");
    })
    .join()
    .expect("the thread finished");
}

/// Descending uses stack, and the measurement follows it down.
#[test]
fn headroom_falls_as_the_stack_is_spent() {
    std::thread::Builder::new()
        .stack_size(4 * 1024 * 1024)
        .spawn(|| {
            mark(4 * 1024 * 1024);
            let top = headroom().expect("marked");

            fn descend(depth: usize) -> usize {
                // An array per frame, so each level costs something the optimiser cannot fold away.
                let ballast = [0u8; 4096];
                std::hint::black_box(&ballast);
                match depth {
                    0 => headroom().expect("marked"),
                    _ => descend(depth - 1),
                }
            }
            let deep = descend(100);

            assert!(deep < top, "{deep} is not below {top}");
            assert!(
                top - deep >= 100 * 4096,
                "100 frames of 4 KiB should show: {top} -> {deep}"
            );
        })
        .expect("spawned")
        .join()
        .expect("the thread finished");
}

/// A thread with less stack than the reserve is exhausted from the start, which is the arithmetic
/// working rather than a case to hit: it is what stops a small thread descending at all.
#[test]
fn a_stack_smaller_than_the_reserve_is_already_exhausted() {
    std::thread::spawn(|| {
        mark(RESERVE / 2);
        assert!(exhausted(), "half a reserve is not enough to go on with");
    })
    .join()
    .expect("the thread finished");
}

/// A roomy thread is not refused, which is every real shell at every real depth.
#[test]
fn a_fresh_stack_is_not_exhausted() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            mark(16 * 1024 * 1024);
            assert!(!exhausted());
            assert!(headroom().expect("marked") > 8 * 1024 * 1024);
        })
        .expect("spawned")
        .join()
        .expect("the thread finished");
}

/// The bounds are per thread: one thread's reading must not answer for another's.
#[test]
fn each_thread_measures_its_own() {
    mark(16 * 1024 * 1024);
    let mine = headroom().expect("marked");
    let theirs = std::thread::spawn(headroom)
        .join()
        .expect("the thread finished");
    assert!(mine > 0);
    assert_eq!(theirs, None, "the other thread was never marked");
}
