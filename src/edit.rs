//! A value as the person types it: the text a form holds for each type, and
//! the data that text comes to, or why it doesn't.

use peios::registry::{Data, ValueType};

/// The types a new value may be given, in the order offered. A link, a
/// big-endian number or a value of no type is kept when edited, but not
/// offered: they are written by programs, not by hand.
pub const NEW_TYPES: [ValueType; 6] =
    [ValueType::SZ, ValueType::EXPAND_SZ, ValueType::MULTI_SZ, ValueType::DWORD, ValueType::QWORD, ValueType::BINARY];

/// How a number is typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base {
    Decimal,
    Hex,
}

impl Base {
    pub fn named(name: &str) -> Base {
        if name == "hex" { Base::Hex } else { Base::Decimal }
    }

    pub fn name(self) -> &'static str {
        match self {
            Base::Decimal => "decimal",
            Base::Hex => "hex",
        }
    }
}

/// What the form looks like for a type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// One line of text.
    Line,
    /// One item to a line.
    Lines,
    /// A number, of so many bits, in decimal or hex.
    Number { bits: u32 },
    /// Bytes in hex.
    Bytes,
    /// Nothing to type.
    Nothing,
}

/// The form for data of type `ty`. Data that doesn't fit its type is
/// edited as bytes, whatever the type says, so nothing is lost.
pub fn form(ty: ValueType, misfit: bool) -> Form {
    if misfit {
        return Form::Bytes;
    }
    match ty {
        ValueType::SZ | ValueType::EXPAND_SZ | ValueType::LINK => Form::Line,
        ValueType::MULTI_SZ => Form::Lines,
        ValueType::DWORD | ValueType::DWORD_BIG_ENDIAN => Form::Number { bits: 32 },
        ValueType::QWORD => Form::Number { bits: 64 },
        ValueType::NONE => Form::Nothing,
        _ => Form::Bytes,
    }
}

/// What the form holds to begin with for `data`.
pub fn text(data: &Data) -> String {
    match data {
        Data::Sz(text) | Data::ExpandSz(text) | Data::Link(text) => text.clone(),
        Data::MultiSz(list) => list.join("\n"),
        Data::Dword(n) | Data::DwordBigEndian(n) => n.to_string(),
        Data::Qword(n) => n.to_string(),
        Data::Binary(bytes) | Data::Raw(_, bytes) => crate::words::hex(bytes),
        Data::None => String::new(),
    }
}

/// The data that `typed`, in the form for `ty`, comes to, or why it doesn't.
pub fn parse(ty: ValueType, misfit: bool, typed: &str, base: Base) -> Result<Data, String> {
    match form(ty, misfit) {
        Form::Line => {
            if typed.contains('\0') {
                return Err("Text can't have a null character in it.".into());
            }
            let text = typed.to_string();
            Ok(match ty {
                ValueType::EXPAND_SZ => Data::ExpandSz(text),
                ValueType::LINK => Data::Link(text),
                _ => Data::Sz(text),
            })
        }
        Form::Lines => {
            if typed.contains('\0') {
                return Err("Text can't have a null character in it.".into());
            }
            Ok(Data::MultiSz(typed.lines().map(|line| line.trim_end_matches('\r')).filter(|line| !line.is_empty()).map(str::to_string).collect()))
        }
        Form::Number { bits } => {
            let n = number(typed, base, bits)?;
            Ok(match ty {
                ValueType::DWORD_BIG_ENDIAN => Data::DwordBigEndian(n as u32),
                ValueType::QWORD => Data::Qword(n),
                _ => Data::Dword(n as u32),
            })
        }
        Form::Bytes => {
            let bytes = bytes(typed)?;
            Ok(if ty == ValueType::BINARY { Data::Binary(bytes) } else { Data::Raw(ty, bytes) })
        }
        Form::Nothing => Ok(Data::None),
    }
}

fn number(typed: &str, base: Base, bits: u32) -> Result<u64, String> {
    let typed = typed.trim().replace(['_', ','], "");
    let most = if bits == 32 { u64::from(u32::MAX) } else { u64::MAX };
    let parsed = match base {
        Base::Decimal => typed.parse::<u64>().ok(),
        Base::Hex => u64::from_str_radix(typed.strip_prefix("0x").or_else(|| typed.strip_prefix("0X")).unwrap_or(&typed), 16).ok(),
    };
    parsed.filter(|n| *n <= most).ok_or_else(|| match base {
        Base::Decimal => format!("Type a whole number from 0 to {most}."),
        Base::Hex => format!("Type a hexadecimal number from 0 to {most:#x}."),
    })
}

fn bytes(typed: &str) -> Result<Vec<u8>, String> {
    let digits: String = typed.chars().filter(|c| !c.is_whitespace() && !matches!(c, ':' | '-' | ',')).collect();
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("Type bytes as hexadecimal digits, two to a byte, such as 0a ff 12.".into());
    }
    if !digits.len().is_multiple_of(2) {
        return Err("Each byte is two hexadecimal digits, and one is missing.".into());
    }
    Ok((0..digits.len()).step_by(2).map(|at| u8::from_str_radix(&digits[at..at + 2], 16).unwrap_or_default()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_typed_comes_to_data_of_its_type() {
        assert_eq!(parse(ValueType::SZ, false, "dark", Base::Decimal), Ok(Data::Sz("dark".into())));
        assert_eq!(parse(ValueType::EXPAND_SZ, false, "%HOME%", Base::Decimal), Ok(Data::ExpandSz("%HOME%".into())));
        assert_eq!(parse(ValueType::MULTI_SZ, false, "a\r\n\nb\n", Base::Decimal), Ok(Data::MultiSz(vec!["a".into(), "b".into()])));
        assert_eq!(parse(ValueType::DWORD, false, " 4,096 ", Base::Decimal), Ok(Data::Dword(4096)));
        assert_eq!(parse(ValueType::DWORD, false, "0x1000", Base::Hex), Ok(Data::Dword(4096)));
        assert_eq!(parse(ValueType::DWORD_BIG_ENDIAN, false, "1", Base::Decimal), Ok(Data::DwordBigEndian(1)));
        assert_eq!(parse(ValueType::QWORD, false, "9000000000", Base::Decimal), Ok(Data::Qword(9_000_000_000)));
        assert_eq!(parse(ValueType::BINARY, false, "de:ad be-ef\n00", Base::Decimal), Ok(Data::Binary(vec![0xde, 0xad, 0xbe, 0xef, 0])));
        assert_eq!(parse(ValueType::NONE, false, "", Base::Decimal), Ok(Data::None));
    }

    #[test]
    fn what_does_not_fit_says_why() {
        assert_eq!(parse(ValueType::DWORD, false, "4294967296", Base::Decimal), Err("Type a whole number from 0 to 4294967295.".into()));
        assert_eq!(parse(ValueType::DWORD, false, "-1", Base::Decimal), Err("Type a whole number from 0 to 4294967295.".into()));
        assert!(parse(ValueType::QWORD, false, "zz", Base::Hex).unwrap_err().starts_with("Type a hexadecimal number"));
        assert!(parse(ValueType::BINARY, false, "abc", Base::Decimal).unwrap_err().contains("one is missing"));
        assert!(parse(ValueType::BINARY, false, "xy", Base::Decimal).unwrap_err().contains("hexadecimal digits"));
        assert!(parse(ValueType::SZ, false, "a\0b", Base::Decimal).is_err());
    }

    #[test]
    fn data_that_does_not_fit_its_type_is_edited_as_bytes_and_keeps_its_type() {
        assert_eq!(form(ValueType::DWORD, true), Form::Bytes);
        assert_eq!(parse(ValueType::DWORD, true, "01 02 03", Base::Decimal), Ok(Data::Raw(ValueType::DWORD, vec![1, 2, 3])));
        assert_eq!(text(&Data::Raw(ValueType::SZ, vec![0xff])), "ff");
    }

    #[test]
    fn a_form_starts_with_what_the_value_holds() {
        assert_eq!(text(&Data::MultiSz(vec!["a".into(), "b".into()])), "a\nb");
        assert_eq!(text(&Data::Dword(7)), "7");
        let data = Data::Binary(vec![1, 0xab]);
        assert_eq!(parse(ValueType::BINARY, false, &text(&data), Base::Decimal), Ok(data));
    }
}
