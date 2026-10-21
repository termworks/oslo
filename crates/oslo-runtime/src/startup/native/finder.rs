//! Opening the full-screen history finder from the prompt.

use super::{fire, hooks, settings};

/// Open the full-screen history finder.
///
/// `None` means it could not open at all — no terminal, no store, nothing remembered — and
/// the caller should carry on as though it had never been asked. That is a different answer
/// from `Some(Cancelled)`, which means the user looked and declined, and where carrying on
/// would scroll their line away as if Esc had done something.
pub(super) fn open_finder(seed: &str) -> Option<oslo_ui::finder::Outcome> {
    let settings = settings::current();
    if !settings.finder.enabled || !oslo_base::feature::on(oslo_base::feature::at::FINDER) {
        return None;
    }
    let track = oslo_base::track::store()?;
    // **The one read that has to be current.** The command you just ran is written on the
    // writer thread, so for a moment after the prompt returns the store does not have it yet
    // — and asking for your history and not finding the thing you just typed is the kind of
    // wrong that makes a shell feel broken. Worse: with nothing else in the store the finder
    // declines to open at all.
    //
    // Waited for here and nowhere else. This is a key somebody deliberately pressed, so a
    // fraction of a millisecond is invisible; the ghost and the `cd` ranking read the same
    // store on every keystroke and must not, which is the whole point of the writes being off
    // this thread.
    oslo_base::track::writer::settle();
    // Only this language's commands. The editor's history holds both, and offering a Lua line
    // at a shell prompt produces something that cannot run — the same crossing the ghost
    // suggestion and the arrow keys are already filtered for.
    let language = oslo_ui::prompt::language().unwrap_or_else(|| "sh".to_string());
    let commands: Vec<_> = track
        .commands(settings.finder.limit)
        .into_iter()
        .filter(|command| command.mode == language)
        .collect();
    if commands.is_empty() {
        return None;
    }
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // The three history events bracket the search itself. `open` is fired here rather than
    // inside the finder because everything above can decline — no store, nothing remembered,
    // disabled by config — and a hook that fired for a search that never appeared would be
    // lying.
    fire(hooks::at::HISTORY_OPEN, &[("seed", seed)]);
    let outcome = oslo_ui::finder::open(
        &commands,
        &cwd,
        now,
        settings.completion.fuzzy,
        seed,
        settings.finder.scope,
    );
    match &outcome {
        Some(oslo_ui::finder::Outcome::Chosen { line, .. }) => {
            fire(hooks::at::HISTORY_SELECT, &[("line", line)]);
            fire(hooks::at::HISTORY_CLOSE, &[("chosen", "true")]);
        }
        _ => fire(hooks::at::HISTORY_CLOSE, &[("chosen", "false")]),
    }
    outcome
}
