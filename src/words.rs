//! What the window says about values and keys, in words.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use peios::registry::{Data, ValueType};

/// A value's name as it is shown: the default value has none of its own.
pub fn value_name(name: &str) -> &str {
    if name.is_empty() { "(Default)" } else { name }
}

/// What a type is, in words.
pub fn kind(ty: ValueType) -> String {
    let said = match ty {
        ValueType::SZ => "Text",
        ValueType::EXPAND_SZ => "Text with variables",
        ValueType::LINK => "Link",
        ValueType::MULTI_SZ => "List of text",
        ValueType::DWORD => "Number",
        ValueType::DWORD_BIG_ENDIAN => "Number, big-endian",
        ValueType::QWORD => "Large number",
        ValueType::BINARY => "Bytes",
        ValueType::NONE => "No type",
        _ => return ty.name().map_or_else(|| format!("Type {:#x}", ty.0), str::to_owned),
    };
    said.into()
}

/// The type's `REG_*` name, which `reg` and the documentation use.
pub fn type_name(ty: ValueType) -> String {
    ty.name().map_or_else(|| format!("REG({:#x})", ty.0), str::to_owned)
}

/// The most bytes shown on a value's line in the list.
const LINE_BYTES: usize = 24;

/// A value's data on one line, for the list.
pub fn line(data: &Data) -> String {
    match data {
        Data::Sz(text) | Data::ExpandSz(text) | Data::Link(text) => text.clone(),
        Data::MultiSz(list) => list.join(" · "),
        Data::Dword(n) | Data::DwordBigEndian(n) => format!("{n} ({n:#010x})"),
        Data::Qword(n) => format!("{n} ({n:#018x})"),
        Data::Binary(bytes) | Data::Raw(_, bytes) => {
            let shown = hex(&bytes[..bytes.len().min(LINE_BYTES)]);
            if bytes.len() > LINE_BYTES { format!("{shown} …") } else { shown }
        }
        Data::None => String::new(),
    }
}

/// Why a value's data is shown as bytes when its type says otherwise.
pub fn misfit(data: &Data) -> Option<String> {
    match data {
        Data::Raw(ty, _) if *ty == ValueType::NONE => Some("A value of no type should hold nothing, and this holds bytes.".into()),
        Data::Raw(ty, _) if matches!(*ty, ValueType::SZ | ValueType::EXPAND_SZ | ValueType::LINK | ValueType::MULTI_SZ) => {
            Some(format!("Its type is {}, but it is not text: shown as bytes.", type_name(*ty)))
        }
        Data::Raw(ty, bytes) if matches!(*ty, ValueType::DWORD | ValueType::DWORD_BIG_ENDIAN | ValueType::QWORD) => {
            let wants = if *ty == ValueType::QWORD { 8 } else { 4 };
            Some(format!("Its type is {}, which is {wants} bytes, but it holds {}: shown as bytes.", type_name(*ty), bytes_count(bytes.len())))
        }
        _ => None,
    }
}

/// Bytes as two hex digits each, a space between.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect::<Vec<_>>().join(" ")
}

/// How many bytes a row of a dump holds: as many as fit the details pane.
const DUMP_ROW: usize = 8;

/// Bytes as a dump: an offset, then a row of bytes.
pub fn dump(bytes: &[u8]) -> Vec<(String, String)> {
    bytes.chunks(DUMP_ROW).enumerate().map(|(row, chunk)| (format!("{:04x}", row * DUMP_ROW), hex(chunk))).collect()
}

pub fn bytes_count(count: usize) -> String {
    if count == 1 { "1 byte".into() } else { format!("{count} bytes") }
}

/// `count` of `one`, with an s for more than one.
pub fn count(count: usize, one: &str) -> String {
    if count == 1 { format!("1 {one}") } else { format!("{count} {one}s") }
}

/// A time given in nanoseconds since 1970, on this machine's clock.
pub fn when(nanoseconds: u64, zone: &TimeZone) -> String {
    let Ok(at) = Timestamp::from_nanosecond(i128::from(nanoseconds)) else { return "an unknown time".into() };
    at.to_zoned(zone.clone()).strftime("%-d %B %Y, %H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_put_on_a_line_by_their_type() {
        assert_eq!(line(&Data::Sz("dark".into())), "dark");
        assert_eq!(line(&Data::MultiSz(vec!["a".into(), "b".into()])), "a · b");
        assert_eq!(line(&Data::Dword(4096)), "4096 (0x00001000)");
        assert_eq!(line(&Data::Qword(1)), "1 (0x0000000000000001)");
        assert_eq!(line(&Data::Binary(vec![0xde, 0xad])), "de ad");
        assert_eq!(line(&Data::Binary(vec![0; 30])).matches("00").count(), LINE_BYTES);
        assert!(line(&Data::Binary(vec![0; 30])).ends_with(" …"));
        assert_eq!(line(&Data::None), "");
    }

    #[test]
    fn bytes_that_do_not_fit_their_type_say_why() {
        assert_eq!(misfit(&Data::Sz("x".into())), None);
        assert_eq!(misfit(&Data::Raw(ValueType::SZ, vec![0xff])).unwrap(), "Its type is REG_SZ, but it is not text: shown as bytes.");
        assert_eq!(
            misfit(&Data::Raw(ValueType::DWORD, vec![1, 2, 3])).unwrap(),
            "Its type is REG_DWORD, which is 4 bytes, but it holds 3 bytes: shown as bytes."
        );
        assert_eq!(misfit(&Data::Raw(ValueType(0x99), vec![1])), None);
    }

    #[test]
    fn types_are_said_in_words_and_by_name() {
        assert_eq!(kind(ValueType::MULTI_SZ), "List of text");
        assert_eq!(kind(ValueType::RESOURCE_LIST), "REG_RESOURCE_LIST");
        assert_eq!(kind(ValueType(0x99)), "Type 0x99");
        assert_eq!(type_name(ValueType(0x99)), "REG(0x99)");
        assert_eq!(value_name(""), "(Default)");
    }

    #[test]
    fn bytes_are_dumped_eight_to_a_row() {
        let rows = dump(&[7; 17]);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2], ("0010".to_string(), "07".to_string()));
    }

    #[test]
    fn times_are_said_on_the_clock_given() {
        assert_eq!(when(1_000_000_000, &TimeZone::UTC), "1 January 1970, 00:00:01");
    }
}
