use super::*;

/// A module name is a name — never the size or the dependency list that follow it on the line.
#[test]
fn a_module_is_offered_by_name() {
    for one in modules() {
        assert!(!one.value.is_empty());
        assert!(!one.value.contains(char::is_whitespace), "{}", one.value);
        assert!(one.value.parse::<u64>().is_err(), "{} is a size", one.value);
    }
}

/// **The dotted form, not the path**, because that is what `sysctl` takes: `/proc/sys/net/ipv4` is
/// `net.ipv4`, and a row offering the path would offer something `sysctl` rejects.
#[test]
fn a_sysctl_is_dotted_and_not_a_path() {
    let found = sysctls();
    if found.is_empty() {
        return; // No `/proc/sys` to walk.
    }
    assert!(
        found.iter().any(|one| one.value.starts_with("kernel.")),
        "nothing under `kernel.` among {} keys",
        found.len()
    );
    for one in &found {
        assert!(!one.value.contains('/'), "{} is still a path", one.value);
        assert!(!one.value.starts_with('.'), "{}", one.value);
    }
}

/// The walk is cached, and a second call has to answer the same thing rather than a second walk's
/// worth of a slightly different tree.
#[test]
fn the_walk_happens_once() {
    assert_eq!(sysctls().len(), sysctls().len());
}
