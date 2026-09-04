//! A pipeline value as Lua sees it, and a Lua value as a pipeline sees it.
//!
//! **One converter each way, because there were three and they disagreed.** A cell crossed here
//! whenever `where` bound a row's columns as globals, whenever `oslo.register_tool` handed a tool
//! its input, and whenever `ps` and `ls` read the rows their Lua helpers had already built — and
//! each of those sites had written the crossing out again:
//!
//! | cell | to a filter | to a Lua tool | from `sh.ps`, `sh.ls` |
//! |---|---|---|---|
//! | [`Val::Bytes`] | its length, a number | a lossy string | — |
//! | [`Val::Error`] | `nil` | the text `error: …` | — |
//! | a Lua table | — | its shape, kept | `nil`, the cell lost |
//! | a Lua byte string | — | `nil`, the cell lost | `nil`, the cell lost |
//!
//! A filter and the tool it feeds are two halves of one pipeline — `blobs \| where '…' \| redact`
//! passed the same cell to both — so they cannot hold different things. Nor can the two directions
//! disagree with each other: a tool that does nothing but hand its input back has to hand back what
//! it was given, and every arm missing from one side was a cell that such a tool destroyed.
//!
//! # The rule
//!
//! A cell reaches Lua as the nearest thing Lua actually has, and never as its *rendering*: a
//! [`Val::Size`] is a count of bytes and a [`Val::Duration`] a count of nanoseconds, so
//! `free < 1e9` is arithmetic. Handing over `4.2G` would make every comparison a string comparison
//! and every filter quietly wrong, which is the whole reason a size is a distinct kind.
//!
//! The two that were in dispute:
//!
//! * [`Val::Bytes`] is a **byte string**, through [`oslo_base::value::Value::bytes`] — the
//!   constructor that keeps text
//!   as text and anything else as the bytes themselves. Lua has byte strings, so there is no reason
//!   to lose the content: `from_utf8_lossy` is the mojibake `Val::Bytes` exists to prevent, and a
//!   length is the blob thrown away.
//! * [`Val::Error`] is a **table** `{ error = "…" }`. It has to be distinguishable from every other
//!   cell, and `nil` and a plain string are not: `nil` is already what an absent column is, and a
//!   string is already what a column of text is — so a filter could not tell a cell that failed
//!   from one that legitimately held that value. It is the shape `to json` gives the same cell.
//!
//! # A cell may name its own kind
//!
//! An error used to come back as a [`Val::Record`] of one field, so a tool that handed its input
//! straight back turned a failure into data — `<error: 42>` on the way in, `error: 42` on the way
//! out. It round-trips now: `{ error = … }` is read as the error it is.
//!
//! A size, a duration and a time cannot round-trip *as numbers*, because a number carries no kind
//! and making it carry one would cost the arithmetic above. So there is a way to **write** them
//! instead: a one-key table naming the kind — `{ __size = 4509715660 }` — which `oslo.rows.size`,
//! `duration`, `time` and `fail` produce. Until that existed, four of the eleven kinds were things
//! only Rust could make, and a tool a config registered could not answer with a size that drew like
//! `df`'s.
//!
//! See `tagged_kind` for what is recognised, and what it costs.

use crate::data::{Record, Val};
use oslo_base::value::{Table, Value};

/// One cell, as Lua sees it.
pub fn to_lua(value: &Val) -> Value {
    match value {
        Val::Null => Value::Nil,
        Val::Bool(b) => Value::Bool(*b),
        Val::Int(i) => Value::int(*i),
        Val::Float(f) => Value::float(*f),
        Val::Str(s) => Value::str(s),
        Val::Bytes(b) => Value::bytes(b),
        // Values, not renderings: bytes and nanoseconds, so a comparison means what it looks like.
        Val::Size(bytes) => Value::int(*bytes as i64),
        Val::Duration(nanos) | Val::Time(nanos) => Value::int(*nanos),
        Val::List(items) => {
            let mut list = Table::new();
            for (i, item) in items.iter().enumerate() {
                list.set(Value::int(i as i64 + 1), to_lua(item));
            }
            Value::table(list)
        }
        Val::Record(record) => Value::table(record_table(record)),
        Val::Error(message) => {
            let mut failed = Table::new();
            failed.set(Value::str("error"), Value::str(message));
            Value::table(failed)
        }
    }
}

/// A record's columns as a Lua table, in the order the record has them.
///
/// The order is not incidental — it decides what `cols` and the drawn table show, and a tool that
/// hands its input back must not silently reorder it.
pub fn record_table(record: &Record) -> Table {
    let mut row = Table::new();
    for (name, value) in record.columns().iter().zip(record.values()) {
        row.set(Value::str(name), to_lua(value));
    }
    row
}

/// Records as a Lua list of tables — the reverse of [`records_of`].
pub fn rows_value(rows: &[Record]) -> Value {
    let mut list = Table::new();
    for (i, record) in rows.iter().enumerate() {
        list.set(Value::int(i as i64 + 1), Value::table(record_table(record)));
    }
    Value::table(list)
}

/// One Lua value as a cell — the reverse of [`to_lua`].
pub fn from_lua(value: &Value) -> Val {
    match value {
        Value::Nil => Val::Null,
        Value::Bool(b) => Val::Bool(*b),
        Value::Number(n) => match n.as_int() {
            Some(i) => Val::Int(i),
            None => Val::Float(n.as_float()),
        },
        Value::Str(s) => Val::Str(s.to_string()),
        Value::Bytes(bytes) => Val::Bytes(bytes.to_vec()),
        Value::Table(table) => table_of(&table.borrow()),
        // A function or a userdata is not a value a row can hold, and there is nothing honest to
        // turn one into.
        _ => Val::Null,
    }
}

/// A Lua list of tables as records — the reverse of [`rows_value`].
///
/// A row that contributes no named column is dropped rather than kept as an empty record, so a list
/// with a stray number in it does not become a blank line in the drawn table.
pub fn records_of(value: &Value) -> Vec<Record> {
    let Value::Table(list) = value else {
        return Vec::new();
    };
    let list = list.borrow();
    let mut out = Vec::new();
    for i in 1..=list.length() {
        let Value::Table(row) = list.get(&Value::int(i)) else {
            continue;
        };
        let mut record = Record::new();
        for (key, value) in row.borrow().pairs() {
            if let Value::Str(name) = key {
                record.set(&name, from_lua(&value));
            }
        }
        if !record.is_empty() {
            out.push(record);
        }
    }
    out
}

/// A cell that names its own kind, or `None` for an ordinary table.
///
/// **Four of the eleven kinds could not be written from Lua at all**, and one of them could not even
/// survive being handed back. `to_lua` gives an error `{ error = "…" }` and this side read it as a
/// record of one field, so `map "{ e = cell }"` turned a failure into data — drawn as `error: 42`
/// where the cell it came from drew `<error: 42>`. A size, a duration and a time reach Lua as plain
/// numbers *on purpose*, so that `free < 1e9` is arithmetic rather than a string comparison; the
/// cost is that a number on the way back cannot say which of the four it was.
///
/// So the way back gains a shape the way out never uses for a size: `{ __size = n }`, written by
/// [`crate::data::lua`]'s callers through `oslo.rows.size` and friends. An error keeps the untagged
/// `{ error = … }` it already had, because that is what `to_lua` writes and the two directions have
/// to agree.
///
/// **A single key, and only these names.** A record that happens to hold one field called `error` is
/// read as a failure — which is the same ambiguity `to_lua` has always had, now merely visible from
/// both ends. The `__` prefix on the other three is what keeps a column called `size` from being
/// mistaken for one.
fn tagged_kind(table: &Table) -> Option<Val> {
    if table.length() >= 1 {
        return None;
    }
    let pairs = table.pairs();
    let [(key, value)] = pairs.as_slice() else {
        return None;
    };
    let Value::Str(name) = key else {
        return None;
    };
    let number = || match &value {
        Value::Number(n) => n.as_int(),
        _ => None,
    };
    match name.as_ref() {
        "error" => match &value {
            Value::Str(message) => Some(Val::Error(message.to_string())),
            other => Some(Val::Error(render_lua(other))),
        },
        "__size" => Some(Val::Size(number()?.max(0) as u64)),
        "__duration" => Some(Val::Duration(number()?)),
        "__time" => Some(Val::Time(number()?)),
        _ => None,
    }
}

/// A non-string error payload as the text of a message.
fn render_lua(value: &Value) -> String {
    match value {
        Value::Number(n) => match n.as_int() {
            Some(i) => i.to_string(),
            None => n.as_float().to_string(),
        },
        Value::Bool(b) => b.to_string(),
        other => other.type_name().to_string(),
    }
}

/// A nested table as a list or a record, the way [`to_lua`] would have written it.
///
/// **The two directions have to agree.** `to_lua` gives [`Val::List`] and [`Val::Record`] an arm
/// each, while this side once read every table as a list of rows and kept the first — so `{ x = 1 }`
/// has length zero, the walk never ran, and the cell came back as `{}`. A tool that did nothing but
/// pass a nested row through destroyed it.
///
/// Lua has one type for both, so the length is the only thing that tells them apart.
fn table_of(table: &Table) -> Val {
    if let Some(tagged) = tagged_kind(table) {
        return tagged;
    }
    if table.length() >= 1 {
        return Val::List(
            (1..=table.length())
                .map(|i| from_lua(&table.get(&Value::int(i))))
                .collect(),
        );
    }
    let mut record = Record::new();
    for (key, value) in table.pairs() {
        if let Value::Str(name) = key {
            record.set(&name, from_lua(&value));
        }
    }
    Val::Record(record)
}

#[cfg(test)]
mod tests {

    /// **The two directions have to agree**, which the module says of itself and did not do.
    ///
    /// Four kinds were lost. A size, a duration and a time reach Lua as plain numbers on purpose, so
    /// the *number* cannot come back as what it was — but a cell written with its kind can, and an
    /// error's `{ error = … }` was being read back as a record of one field, which is a failure
    /// turned into data.
    #[test]
    fn every_kind_survives_being_handed_back() {
        // One value per arm of `Val`, so a kind added later fails here rather than silently
        // joining the ones that used to be lost.
        let kinds = [
            Val::Null,
            Val::Bool(true),
            Val::Int(-7),
            Val::Float(1.5),
            Val::Str("text".into()),
            Val::Bytes(vec![0, 159, 146]),
            Val::List(vec![Val::Int(1), Val::Str("two".into())]),
            Val::Record(Record::from_pairs([("a", Val::Int(1))])),
            Val::Error("stale handle".into()),
        ];
        for value in kinds {
            assert_eq!(
                from_lua(&to_lua(&value)),
                value,
                "{value:?} did not survive the round trip"
            );
        }
    }

    /// The three that reach Lua as numbers, which is the whole reason `free < 1e9` is arithmetic.
    ///
    /// They cannot round-trip *as numbers* — a number carries no kind — so what is asserted is the
    /// other half: a cell written with its kind is read back as that kind.
    #[test]
    fn a_number_kind_is_written_with_its_name_and_read_back() {
        for (name, value, expected) in [
            ("__size", 4_509_715_660i64, Val::Size(4_509_715_660)),
            ("__duration", 1_500_000_000, Val::Duration(1_500_000_000)),
            (
                "__time",
                1_551_744_000_000_000_000,
                Val::Time(1_551_744_000_000_000_000),
            ),
        ] {
            let mut cell = Table::new();
            cell.set(Value::str(name), Value::int(value));
            assert_eq!(from_lua(&Value::table(cell)), expected, "{name}");
        }

        // And a plain number is still a plain number: the tag is the only thing that lifts it.
        assert_eq!(from_lua(&Value::int(2048)), Val::Int(2048));
    }

    /// A record that merely *has* one of the names is not a tagged cell.
    #[test]
    fn a_record_of_two_fields_is_still_a_record() {
        let mut table = Table::new();
        table.set(Value::str("__size"), Value::int(1));
        table.set(Value::str("other"), Value::int(2));
        assert!(matches!(from_lua(&Value::table(table)), Val::Record(_)));
    }
    use super::*;

    fn as_int(value: &Value) -> Option<i64> {
        match value {
            Value::Number(n) => n.as_int(),
            _ => None,
        }
    }

    fn as_text(value: &Value) -> Option<String> {
        match value {
            Value::Str(s) => Some(s.to_string()),
            _ => None,
        }
    }

    /// A size compares as a number, which is exactly what `ls -lh | sort` cannot do.
    #[test]
    fn a_size_is_arithmetic_in_lua() {
        assert_eq!(as_int(&to_lua(&Val::Size(1024))), Some(1024));
        assert_eq!(
            as_int(&to_lua(&Val::Duration(1_500_000_000))),
            Some(1_500_000_000)
        );
    }

    /// Every column reaches Lua under its own name.
    #[test]
    fn a_record_reaches_lua_as_a_table() {
        let record = Record::from_pairs([
            ("mount", Val::Str("/".into())),
            ("free", Val::Size(500_000_000)),
        ]);
        let Value::Table(row) = to_lua(&Val::Record(record)) else {
            panic!("a record is a table");
        };
        let row = row.borrow();
        assert_eq!(as_text(&row.get_str("mount")).as_deref(), Some("/"));
        assert_eq!(as_int(&row.get_str("free")), Some(500_000_000));
    }

    /// **The blob crosses whole.** One converter answered its length and the other a string that
    /// `from_utf8_lossy` had already replaced every invalid sequence in — a JPEG arrived as either
    /// a number or mojibake, and `Val::Bytes` exists precisely so that neither happens.
    #[test]
    fn bytes_cross_as_bytes() {
        let jpeg = vec![0xff, 0xd8, 0xff, 0xe0];
        assert_eq!(
            to_lua(&Val::Bytes(jpeg.clone())).as_bytes(),
            Some(&jpeg[..])
        );

        // Bytes that are text arrive as text, so an ordinary comparison still works.
        let text = to_lua(&Val::Bytes(b"hello".to_vec()));
        assert_eq!(as_text(&text).as_deref(), Some("hello"));
    }

    /// **A failed cell is visible and cannot be mistaken for a value.** `nil` is what an absent
    /// column already is and a string is what a column of text already is, so neither could be
    /// told apart from a cell that legitimately held it.
    #[test]
    fn an_error_is_a_table_that_says_so() {
        let Value::Table(failed) = to_lua(&Val::Error("stale handle".into())) else {
            panic!("an error cell is a table");
        };
        assert_eq!(
            as_text(&failed.borrow().get_str("error")).as_deref(),
            Some("stale handle")
        );
        assert!(
            !matches!(to_lua(&Val::Error("x".into())), Value::Nil),
            "an error is not the same as an absent column"
        );
    }

    /// A list stays a list, all of it, and nesting survives.
    #[test]
    fn a_list_keeps_its_shape() {
        let Value::Table(list) = to_lua(&Val::List(vec![Val::Int(10), Val::Int(20)])) else {
            panic!("a list is a table");
        };
        let list = list.borrow();
        assert_eq!(list.length(), 2);
        assert_eq!(as_int(&list.get(&Value::int(2))), Some(20));
    }

    /// A list of tables becomes rows, with the columns in the order the table had them — which is
    /// the reason the Lua table had to become insertion-ordered first.
    #[test]
    fn a_lua_list_of_tables_becomes_records() {
        let mut row = Table::new();
        row.set(Value::str("host"), Value::str("a"));
        row.set(Value::str("ip"), Value::str("10.0.0.1"));
        let mut list = Table::new();
        list.set(Value::int(1), Value::table(row));

        let records = records_of(&Value::table(list));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].columns(), ["host", "ip"]);
        assert_eq!(records[0].get("ip"), Some(&Val::Str("10.0.0.1".into())));
    }

    /// **A nested table keeps its shape.** One reverse converter dropped every table to `Val::Null`
    /// and the other read them all as lists of rows, keeping only the first — so a map cell, whose
    /// length is zero, arrived as `{}`: `nest | to json` printed `"inner": {}` for `{ x = 1 }`.
    #[test]
    fn a_nested_table_keeps_its_shape() {
        let mut inner = Table::new();
        inner.set(Value::str("x"), Value::int(1));
        let Val::Record(record) = from_lua(&Value::table(inner)) else {
            panic!("a map is a record");
        };
        assert_eq!(record.get("x"), Some(&Val::Int(1)));

        let mut list = Table::new();
        for (i, value) in [10, 20].into_iter().enumerate() {
            list.set(Value::int(i as i64 + 1), Value::int(value));
        }
        assert_eq!(
            from_lua(&Value::table(list)),
            Val::List(vec![Val::Int(10), Val::Int(20)])
        );
    }

    /// The two directions agree: what `to_lua` writes, `from_lua` reads back unchanged.
    #[test]
    fn a_nested_value_survives_the_round_trip() {
        let mut record = Record::new();
        record.set("x", Val::Int(1));
        let nested = Val::Record(record);
        assert_eq!(from_lua(&to_lua(&nested)), nested);

        let list = Val::List(vec![Val::Str("a".into()), Val::Int(2)]);
        assert_eq!(from_lua(&to_lua(&list)), list);
    }

    /// **A blob survives a tool that passes it through.** `to_lua` hands over the bytes themselves
    /// rather than a length or a lossy string, so this side has to read them back as bytes — the
    /// arm was missing from both reverse converters, and they arrived as `Val::Null`.
    #[test]
    fn a_blob_survives_the_round_trip() {
        let jpeg = Val::Bytes(vec![0xff, 0xd8, 0xff, 0xe0]);
        assert_eq!(from_lua(&to_lua(&jpeg)), jpeg);

        // Bytes that *are* text are a Lua string, and come back as one: `Value::bytes` keeps the
        // two variants disjoint, so text never reaches `Value::Bytes` in the first place.
        assert_eq!(
            from_lua(&to_lua(&Val::Bytes(b"hello".to_vec()))),
            Val::Str("hello".to_string())
        );
    }
}
