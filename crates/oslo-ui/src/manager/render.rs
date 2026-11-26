//! Drawing the manager: one full-screen frame per keystroke.
//!
//! Almost none of it is drawn here. The rows, the striping, the columns, the match marks, the
//! three-row surface and the counter all come from [`Preset::History`] — the same look the history
//! finder is drawn with, which is what makes the two screens the same screen. This module decides
//! what the columns *say*:
//!
//! ```text
//!   3d   alias   gs      git status --short              #git #system
//!   └─ when      └─ kind, so a list of four kinds can be read down the column
//!                        └─ the name, then its first line
//!                                                        └─ its tags, hard right
//! ```
//!
//! A macro that is off is drawn muted and marked, because a list where a disabled row looks exactly
//! like a live one is a list that answers the wrong question.

use super::{Item, Source};
use crate::ask::look::{Look, Preset, Row, View};
use crate::paint::{SYNC_BEGIN, SYNC_END};
use crate::theme;

/// Rows the input surface takes: a blank, the query, a blank. The finder's shape, deliberately.
const SURFACE_ROWS: usize = 3;
const CHROME_ROWS: usize = SURFACE_ROWS + 3;
const SCREEN_MARGIN: usize = 1;

pub struct Frame<'a> {
    pub rows: &'a [Item],
    pub selected: usize,
    pub offset: usize,
    pub query: &'a str,
    pub elapsed_ms: u64,
    pub confirm: Option<bool>,
    pub source: Source,
    /// The kind on screen, or `None` for all of them — what ← and → move through.
    pub kind: Option<String>,
    pub total: usize,
    pub cols: usize,
    pub rows_available: usize,
    pub now: i64,
    /// Every tag in use, sorted — what decides the colours. Passed in rather than gathered from
    /// `rows` so that filtering the list, or moving to the other source, does not recolour it.
    pub tags: &'a [String],
}

/// How many rows remain for the list after the input and its margins.
pub(super) fn visible_rows(rows: usize) -> usize {
    rows.saturating_sub(CHROME_ROWS).max(1)
}

/// Unix seconds now, for the age column.
pub(super) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// How long ago, in the two or three characters the column has.
///
/// The finder's `ago` measures a moment in the past few hours; this measures how long you have kept
/// something, which is usually months. Same column, same shape, one more unit on the end.
pub fn ago(now: i64, then: i64) -> String {
    if then <= 0 {
        return "—".to_string();
    }
    let seconds = (now - then).max(0);
    match seconds {
        s if s < 60 => "now".to_string(),
        s if s < 3_600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3_600),
        s if s < 2_592_000 => format!("{}d", s / 86_400),
        s if s < 31_536_000 => format!("{}mo", s / 2_592_000),
        s => format!("{}y", s / 31_536_000),
    }
}

/// The look, with only the two things the preset cannot know set here.
fn look_of(f: &Frame<'_>) -> Look {
    let mut look = Preset::History.look();
    // **Said before the key is pressed**, not after it does nothing: an inherited row has no macro
    // behind it to open or delete, so Enter and Delete are inert there and the status says which
    // list you are in and what that means.
    look.right = match f.source {
        Source::Stored => format!("{} @ {{badge}} || {{n}}/{{total}} ", f.source.label()),
        Source::Elsewhere => format!(
            "{} not editable @ {{badge}} || {{n}}/{{total}} ",
            f.source.label()
        ),
    };
    look.badge = match &f.kind {
        Some(kind) => format!("[{kind}]"),
        None => "[all]".to_string(),
    };
    // **An empty list says which empty list it is.** The screen opens on an empty database on
    // purpose — the database is not the only source, and Tab is how you reach the other one — so the
    // one place a person is already looking says where the rest of it went.
    if f.rows.is_empty() && f.query.is_empty() {
        look.placeholder = match (f.source, f.kind.is_some()) {
            (_, true) => "nothing of this kind — ← → for another".to_string(),
            (Source::Stored, false) => {
                "nothing stored — tab for what your config defines".to_string()
            }
            (Source::Elsewhere, false) => {
                "your config defines none — tab back to what is stored".to_string()
            }
        };
    }
    look
}

pub fn frame(f: &Frame<'_>) -> String {
    let look = look_of(f);
    let visible = visible_rows(f.rows_available);

    // **One column for the name, measured across the whole list** — the same reason the metadata
    // columns are measured that way. Padding each name to its own length puts the bodies a single
    // space behind names of wildly different lengths, which is a ragged left edge where the eye
    // wants a straight one. Capped, so one long name cannot push every body off the screen.
    let name_width = f
        .rows
        .iter()
        .map(|item| item.name.chars().count())
        .max()
        .unwrap_or(0)
        .min(NAME_WIDTH);

    let rows: Vec<Row> = f
        .rows
        .iter()
        .map(|item| row_of(item, &look, f.now, name_width, f.tags))
        .collect();
    let view = View {
        selected: f.selected,
        offset: f.offset,
        height: visible,
        query: f.query,
        matched: f.rows.len(),
        total: f.total,
        marked: 0,
        cols: f.cols,
        filtering: f.confirm.is_none(),
        elapsed_ms: f.elapsed_ms,
    };

    let mut body = look.rows(&rows, &view);
    if let Some(yes) = f.confirm {
        let pager = &theme::current().pager;
        body.extend(std::iter::repeat_n(String::new(), look.gap));
        let question = f
            .rows
            .get(f.selected)
            .map(|item| format!("forget the {} {}?", item.kind, item.name))
            .unwrap_or_else(|| "forget it?".to_string());
        let depth = theme::depth();
        body.extend((0..SURFACE_ROWS).map(|row| {
            crate::finder::render::confirm_row(row, yes, &question, pager, f.cols, depth)
        }));
    }

    let mut out = String::from(SYNC_BEGIN);
    out.push_str("\x1b[H");
    for _ in 0..SCREEN_MARGIN {
        out.push_str("\x1b[2K\r\n");
    }
    for row in &body {
        out.push_str("\x1b[2K");
        out.push_str(row);
        out.push_str("\r\n");
    }
    out.push_str("\x1b[2K");
    out.push_str(SYNC_END);
    out
}

/// One macro as a row of the list.
fn row_of(item: &Item, look: &Look, now: i64, name_width: usize, known: &[String]) -> Row {
    let tags: String = item
        .tags
        .iter()
        .map(|tag| colour_of(tag, known).open(theme::depth()) + "#" + tag)
        .collect::<Vec<_>>()
        .join(" ");
    Row {
        meta: vec![ago(now, item.created), item.kind.clone()],
        lead: lead_of(item).to_string(),
        trail: tags,
        // Muted while it is off: a list where a disabled row looks exactly like a live one answers
        // the wrong question.
        tint: (!item.on()).then_some(look.muted),
        ..Row::new(format!(
            "{:<name_width$}  {}",
            truncated(&item.name, name_width),
            detail(item)
        ))
    }
}

/// The state marker, where the checkbox sits in a multi-select list: off everywhere, off in this
/// session, sharing its name with another kind, or nothing.
fn lead_of(item: &Item) -> &'static str {
    match (item.active, item.session_off, item.clash) {
        (false, _, _) => "✗",
        (true, true, _) => "·",
        (true, false, true) => "!",
        (true, false, false) => " ",
    }
}

/// What sits after the name: an alias's body, or a script's language in brackets.
///
/// **The brackets say it is not the body.** A script row shows `python3` where an alias row shows
/// the command it runs, and without a mark the column reads as though the script's body were the
/// word `python3`. Faint for the same reason: it is a label on the row, not content of it.
///
/// `\x1b[2m` and `\x1b[22m` — faint, then back to normal intensity — rather than a colour, because
/// this has to sit inside whatever the row is already painted with. A colour would have to know the
/// row's own foreground to put it back; intensity is independent of it, so the same two codes work
/// on a striped row, a selected row and a muted one.
fn detail(item: &Item) -> String {
    match item.kind.as_str() {
        "func" | "script" => format!("\x1b[2m[[[ {} ]]]\x1b[22m", item.first),
        _ => item.first.clone(),
    }
}

/// How wide the name column may grow before a body is worth more than an aligned edge.
const NAME_WIDTH: usize = 22;

fn truncated(name: &str, width: usize) -> String {
    match name.chars().count() > width {
        true => {
            name.chars()
                .take(width.saturating_sub(1))
                .collect::<String>()
                + "…"
        }
        false => name.to_string(),
    }
}

/// The colour a tag is drawn in: **its place in the sorted set of tags you use**.
///
/// The obvious answer is to hash the name, so a tag's colour depends on nothing but the tag. It was
/// measured and it does not work: with 15 tags and a palette of 24 a collision is not unlikely, it is
/// near-certain — the birthday problem — and the run that proved it put `net` and `files` on the same
/// blue while leaving 13 colours unused. Two tags the same colour in one list is the whole feature
/// failing, and no palette small enough to stay cool is large enough to make hashing safe.
///
/// By position, every tag on screen is a different colour, guaranteed. The cost is that adding a tag
/// early in the alphabet shifts the ones after it — paid once, when you invent a tag, against a list
/// that is unambiguous every other day.
///
/// The palette is cool on purpose: red and orange in a list mean *wrong*, and a tag is not a warning.
/// They are 256-colour indexes rather than truecolour triples, so a row looks the same over ssh into
/// a terminal that has neither.
fn colour_of(tag: &str, tags: &[String]) -> theme::Style {
    const PALETTE: [u8; 24] = [
        39, 45, 51, 43, 49, 37, 74, 80, 86, 68, 69, 75, 110, 111, 117, 140, 141, 147, 114, 79, 108,
        183, 177, 152,
    ];
    let at = tags.iter().position(|known| known == tag).unwrap_or(0);
    theme::Style {
        fg: Some(theme::Color::Indexed(PALETTE[at % PALETTE.len()])),
        ..theme::Style::default()
    }
}

// The confirmation box is `finder::render::confirm_row`, question and all. Two screens drawing the
// same box from two copies of the same arithmetic would be two boxes that eventually differ.

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(words: &[&str]) -> Vec<String> {
        let mut tags: Vec<String> = words.iter().map(|w| w.to_string()).collect();
        tags.sort();
        tags
    }

    /// **The measurement that decided against hashing, as a test.** These are the fifteen tags a
    /// real macro database ended up with; hashing them into a palette of this size collided four
    /// times and left thirteen colours unused.
    #[test]
    fn every_tag_on_screen_is_a_different_colour() {
        let tags = tags(&[
            "net", "desktop", "shell", "files", "system", "dev", "disk", "docs", "virt", "robot",
            "media", "pkg", "git", "secret", "ai",
        ]);
        let mut colours: Vec<Option<theme::Color>> =
            tags.iter().map(|tag| colour_of(tag, &tags).fg).collect();
        let before = colours.len();
        colours.sort_by_key(|colour| format!("{colour:?}"));
        colours.dedup();
        assert_eq!(colours.len(), before, "two tags share a colour");
    }

    /// The same tag is the same colour in every row, which is the whole point of a colour.
    #[test]
    fn a_tag_keeps_its_colour() {
        let tags = tags(&["git", "net", "shell"]);
        assert_eq!(colour_of("net", &tags).fg, colour_of("net", &tags).fg);
        assert_ne!(colour_of("net", &tags).fg, colour_of("git", &tags).fg);
        // One nobody listed still gets a colour rather than a panic.
        assert!(colour_of("unknown", &tags).fg.is_some());
    }

    fn item(kind: &str, first: &str) -> Item {
        Item {
            kind: kind.to_string(),
            name: "x".to_string(),
            first: first.to_string(),
            tags: Vec::new(),
            created: 0,
            active: true,
            session_off: false,
            stored: true,
            clash: false,
        }
    }

    /// A script says its language, in brackets and faint; an alias says its body, plainly.
    #[test]
    fn only_the_kinds_with_no_body_on_the_row_get_the_brackets() {
        assert_eq!(detail(&item("alias", "git status")), "git status");
        assert_eq!(
            detail(&item("script", "python3")),
            "\x1b[2m[[[ python3 ]]]\x1b[22m"
        );
        assert_eq!(detail(&item("func", "sh")), "\x1b[2m[[[ sh ]]]\x1b[22m");
    }

    /// **Intensity, not colour**, so the label sits inside whatever the row is painted with and
    /// leaves it as it found it — a striped row keeps its stripe and a selected row its selection.
    #[test]
    fn the_faint_label_changes_nothing_but_intensity() {
        let painted = detail(&item("script", "bash"));
        assert!(!painted.contains("38;5;"), "no foreground: {painted:?}");
        assert!(!painted.contains("\x1b[0m"), "no reset: {painted:?}");
        assert_eq!(
            crate::dropdown::width::display_width(&painted),
            "[[[ bash ]]]".chars().count(),
            "the escapes take no width, so the column still lines up"
        );
    }

    /// A clash is marked on a live row; off says off, whatever else is true.
    #[test]
    fn a_clashing_row_is_marked_unless_it_is_off() {
        let clashing = Item {
            clash: true,
            ..item("script", "bash")
        };
        assert_eq!(lead_of(&clashing), "!");
        assert_eq!(lead_of(&item("script", "bash")), " ");
        let off = Item {
            active: false,
            ..clashing
        };
        assert_eq!(lead_of(&off), "✗");
    }

    /// A name longer than the column is cut, not allowed to push every body off the screen.
    #[test]
    fn a_long_name_is_cut_to_the_column() {
        assert_eq!(truncated("gs", 8), "gs");
        assert_eq!(truncated("zz-update-systemd-boot", 10), "zz-update…");
        assert_eq!(
            truncated("gs", 0),
            "…",
            "even at nothing, it says something"
        );
    }

    /// How long you have kept something is months, not minutes — and undated is undated.
    #[test]
    fn the_age_column_covers_years_and_says_when_it_cannot() {
        let now = 100_000_000;
        assert_eq!(ago(now, 0), "—", "no date recorded");
        assert_eq!(ago(now, now - 10), "now");
        assert_eq!(ago(now, now - 3 * 86_400), "3d");
        assert_eq!(ago(now, now - 40 * 86_400), "1mo");
        assert_eq!(ago(now, now - 800 * 86_400), "2y");
    }
}
