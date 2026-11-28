//! `%(fmt)T`: an epoch second through `strftime`, in the zone the exported `TZ` names.

/// Find the `)T` that closes a time format. `text` starts after the `(`; nested parentheses
/// pair up. Returns the format and the bytes used through the `T`, or `None` if it is not closed.
pub fn split(text: &[u8]) -> Option<(&[u8], usize)> {
    let mut depth = 1;
    for (at, &byte) in text.iter().enumerate() {
        match byte {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            return (text.get(at + 1) == Some(&b'T')).then_some((&text[..at], at + 2));
        }
    }
    None
}

/// The formatted time. `arg` is the argument's value: none or `-1` is now, `-2` is when the shell
/// started. An empty format is `%X`, and a second with no local time is the epoch.
pub fn render(format: &[u8], arg: Option<i64>) -> Vec<u8> {
    let now = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64)
    };
    let seconds = match arg {
        None | Some(-1) => now(),
        Some(-2) => crate::env::dynamic::started() as i64,
        Some(given) => given,
    };
    let format = if format.is_empty() {
        b"%X".as_slice()
    } else {
        format
    };
    oslo_base::clock::format_at(seconds, format)
        .or_else(|| oslo_base::clock::format_at(0, format))
        .unwrap_or_default()
}
