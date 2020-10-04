// Copyright 2013-2014 The rust-url developers.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! [*Unicode IDNA Compatibility Processing*
//! (Unicode Technical Standard #46)](http://www.unicode.org/reports/tr46/)

use self::Mapping::*;
use crate::punycode;
use std::cmp::Ordering::{Equal, Greater, Less};
use std::{error::Error as StdError, fmt};
use unicode_bidi::{bidi_class, BidiClass};
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::{is_nfc, UnicodeNormalization};

include!("uts46_mapping_table.rs");

const PUNYCODE_PREFIX: &str = "xn--";

#[derive(Debug)]
struct StringTableSlice {
    // Store these as separate fields so the structure will have an
    // alignment of 1 and thus pack better into the Mapping enum, below.
    byte_start_lo: u8,
    byte_start_hi: u8,
    byte_len: u8,
}

fn decode_slice(slice: &StringTableSlice) -> &'static str {
    let lo = slice.byte_start_lo as usize;
    let hi = slice.byte_start_hi as usize;
    let start = (hi << 8) | lo;
    let len = slice.byte_len as usize;
    &STRING_TABLE[start..(start + len)]
}

#[repr(u8)]
#[derive(Debug)]
enum Mapping {
    Valid,
    Ignored,
    Mapped(StringTableSlice),
    Deviation(StringTableSlice),
    Disallowed,
    DisallowedStd3Valid,
    DisallowedStd3Mapped(StringTableSlice),
}

struct Range {
    from: char,
    to: char,
}

fn find_char(codepoint: char) -> &'static Mapping {
    let r = TABLE.binary_search_by(|ref range| {
        if codepoint > range.to {
            Less
        } else if codepoint < range.from {
            Greater
        } else {
            Equal
        }
    });
    r.ok()
        .map(|i| {
            const SINGLE_MARKER: u16 = 1 << 15;

            let x = INDEX_TABLE[i];
            let single = (x & SINGLE_MARKER) != 0;
            let offset = !SINGLE_MARKER & x;

            if single {
                &MAPPING_TABLE[offset as usize]
            } else {
                &MAPPING_TABLE[(offset + (codepoint as u16 - TABLE[i].from as u16)) as usize]
            }
        })
        .unwrap()
}

fn map_char(codepoint: char, config: Config, output: &mut String, errors: &mut Errors) {
    if let '.' | '-' | 'a'..='z' | '0'..='9' = codepoint {
        output.push(codepoint);
        return;
    }

    match *find_char(codepoint) {
        Mapping::Valid => output.push(codepoint),
        Mapping::Ignored => {}
        Mapping::Mapped(ref slice) => output.push_str(decode_slice(slice)),
        Mapping::Deviation(ref slice) => {
            if config.transitional_processing {
                output.push_str(decode_slice(slice))
            } else {
                output.push(codepoint)
            }
        }
        Mapping::Disallowed => {
            errors.disallowed_character = true;
            output.push(codepoint);
        }
        Mapping::DisallowedStd3Valid => {
            if config.use_std3_ascii_rules {
                errors.disallowed_by_std3_ascii_rules = true;
            }
            output.push(codepoint)
        }
        Mapping::DisallowedStd3Mapped(ref slice) => {
            if config.use_std3_ascii_rules {
                errors.disallowed_mapped_in_std3 = true;
            }
            output.push_str(decode_slice(slice))
        }
    }
}

// http://tools.ietf.org/html/rfc5893#section-2
fn passes_bidi(label: &str, is_bidi_domain: bool) -> bool {
    // Rule 0: Bidi Rules apply to Bidi Domain Names: a name with at least one RTL label.  A label
    // is RTL if it contains at least one character of bidi class R, AL or AN.
    if !is_bidi_domain {
        return true;
    }

    let mut chars = label.chars();
    let first_char_class = match chars.next() {
        Some(c) => bidi_class(c),
        None => return true, // empty string
    };

    match first_char_class {
        // LTR label
        BidiClass::L => {
            // Rule 5
            while let Some(c) = chars.next() {
                if !matches!(
                    bidi_class(c),
                    BidiClass::L
                        | BidiClass::EN
                        | BidiClass::ES
                        | BidiClass::CS
                        | BidiClass::ET
                        | BidiClass::ON
                        | BidiClass::BN
                        | BidiClass::NSM
                ) {
                    return false;
                }
            }

            // Rule 6
            // must end in L or EN followed by 0 or more NSM
            let mut rev_chars = label.chars().rev();
            let mut last_non_nsm = rev_chars.next();
            loop {
                match last_non_nsm {
                    Some(c) if bidi_class(c) == BidiClass::NSM => {
                        last_non_nsm = rev_chars.next();
                        continue;
                    }
                    _ => {
                        break;
                    }
                }
            }
            match last_non_nsm {
                Some(c) if bidi_class(c) == BidiClass::L || bidi_class(c) == BidiClass::EN => {}
                Some(_) => {
                    return false;
                }
                _ => {}
            }
        }

        // RTL label
        BidiClass::R | BidiClass::AL => {
            let mut found_en = false;
            let mut found_an = false;

            // Rule 2
            for c in chars {
                let char_class = bidi_class(c);
                if char_class == BidiClass::EN {
                    found_en = true;
                } else if char_class == BidiClass::AN {
                    found_an = true;
                }

                if !matches!(
                    char_class,
                    BidiClass::R
                        | BidiClass::AL
                        | BidiClass::AN
                        | BidiClass::EN
                        | BidiClass::ES
                        | BidiClass::CS
                        | BidiClass::ET
                        | BidiClass::ON
                        | BidiClass::BN
                        | BidiClass::NSM
                ) {
                    return false;
                }
            }
            // Rule 3
            let mut rev_chars = label.chars().rev();
            let mut last = rev_chars.next();
            loop {
                // must end in L or EN followed by 0 or more NSM
                match last {
                    Some(c) if bidi_class(c) == BidiClass::NSM => {
                        last = rev_chars.next();
                        continue;
                    }
                    _ => {
                        break;
                    }
                }
            }
            match last {
                Some(c)
                    if matches!(
                        bidi_class(c),
                        BidiClass::R | BidiClass::AL | BidiClass::EN | BidiClass::AN
                    ) => {}
                _ => {
                    return false;
                }
            }

            // Rule 4
            if found_an && found_en {
                return false;
            }
        }

        // Rule 1: Should start with L or R/AL
        _ => {
            return false;
        }
    }

    true
}

/// Check the validity criteria for the given label
///
/// V1 (NFC) and V8 (Bidi) are checked inside `processing()` to prevent doing duplicate work.
///
/// http://www.unicode.org/reports/tr46/#Validity_Criteria
fn is_valid(label: &str, config: Config) -> bool {
    let first_char = label.chars().next();
    if first_char == None {
        // Empty string, pass
        return true;
    }

    // V2: No U+002D HYPHEN-MINUS in both third and fourth positions.
    //
    // NOTE: Spec says that the label must not contain a HYPHEN-MINUS character in both the
    // third and fourth positions. But nobody follows this criteria. See the spec issue below:
    // https://github.com/whatwg/url/issues/53

    // V3: neither begin nor end with a U+002D HYPHEN-MINUS
    if config.check_hyphens && (label.starts_with('-') || label.ends_with('-')) {
        return false;
    }

    // V4: not contain a U+002E FULL STOP
    //
    // Here, label can't contain '.' since the input is from .split('.')

    // V5: not begin with a GC=Mark
    if is_combining_mark(first_char.unwrap()) {
        return false;
    }

    // V6: Check against Mapping Table
    if label.chars().any(|c| match *find_char(c) {
        Mapping::Valid => false,
        Mapping::Deviation(_) => config.transitional_processing,
        Mapping::DisallowedStd3Valid => config.use_std3_ascii_rules,
        _ => true,
    }) {
        return false;
    }

    // V7: ContextJ rules
    //
    // TODO: Implement rules and add *CheckJoiners* flag.

    // V8: Bidi rules are checked inside `processing()`
    true
}

/// http://www.unicode.org/reports/tr46/#Processing
fn processing(domain: &str, config: Config) -> (String, Errors) {
    // Weed out the simple cases: only allow all lowercase ASCII characters and digits where none
    // of the labels start with PUNYCODE_PREFIX and labels don't start or end with hyphen.
    let (mut prev, mut simple, mut puny_prefix) = ('?', !domain.is_empty(), 0);
    for c in domain.chars() {
        if c == '.' {
            if prev == '-' {
                simple = false;
                break;
            }
            puny_prefix = 0;
            continue;
        } else if puny_prefix == 0 && c == '-' {
            simple = false;
            break;
        } else if puny_prefix < 5 {
            if c == ['x', 'n', '-', '-'][puny_prefix] {
                puny_prefix += 1;
                if puny_prefix == 4 {
                    simple = false;
                    break;
                }
            } else {
                puny_prefix = 5;
            }
        }
        if !c.is_ascii_lowercase() && !c.is_ascii_digit() {
            simple = false;
            break;
        }
        prev = c;
    }
    if simple {
        return (domain.to_owned(), Errors::default());
    }

    let mut errors = Errors::default();
    let mut mapped = String::with_capacity(domain.len());
    for c in domain.chars() {
        map_char(c, config, &mut mapped, &mut errors)
    }
    let mut normalized = String::with_capacity(mapped.len());
    normalized.extend(mapped.nfc());

    let mut validated = String::new();
    let non_transitional = config.transitional_processing(false);
    let (mut first, mut valid, mut has_bidi_labels) = (true, true, false);
    for label in normalized.split('.') {
        if !first {
            validated.push('.');
        }
        first = false;
        if label.starts_with(PUNYCODE_PREFIX) {
            match punycode::decode_to_string(&label[PUNYCODE_PREFIX.len()..]) {
                Some(decoded_label) => {
                    if !has_bidi_labels {
                        has_bidi_labels |= is_bidi_domain(&decoded_label);
                    }

                    if valid
                        && (!is_nfc(&decoded_label) || !is_valid(&decoded_label, non_transitional))
                    {
                        valid = false;
                    }
                    validated.push_str(&decoded_label)
                }
                None => {
                    has_bidi_labels = true;
                    errors.punycode = true;
                }
            }
        } else {
            if !has_bidi_labels {
                has_bidi_labels |= is_bidi_domain(label);
            }

            // `normalized` is already `NFC` so we can skip that check
            valid &= is_valid(label, config);
            validated.push_str(label)
        }
    }

    for label in validated.split('.') {
        // V8: Bidi rules
        //
        // TODO: Add *CheckBidi* flag
        if !passes_bidi(label, has_bidi_labels) {
            valid = false;
            break;
        }
    }

    if !valid {
        errors.validity_criteria = true;
    }

    (validated, errors)
}

#[derive(Clone, Copy)]
pub struct Config {
    use_std3_ascii_rules: bool,
    transitional_processing: bool,
    verify_dns_length: bool,
    check_hyphens: bool,
}

/// The defaults are that of https://url.spec.whatwg.org/#idna
impl Default for Config {
    fn default() -> Self {
        Config {
            use_std3_ascii_rules: false,
            transitional_processing: false,
            check_hyphens: false,
            // check_bidi: true,
            // check_joiners: true,

            // Only use for to_ascii, not to_unicode
            verify_dns_length: false,
        }
    }
}

impl Config {
    #[inline]
    pub fn use_std3_ascii_rules(mut self, value: bool) -> Self {
        self.use_std3_ascii_rules = value;
        self
    }

    #[inline]
    pub fn transitional_processing(mut self, value: bool) -> Self {
        self.transitional_processing = value;
        self
    }

    #[inline]
    pub fn verify_dns_length(mut self, value: bool) -> Self {
        self.verify_dns_length = value;
        self
    }

    #[inline]
    pub fn check_hyphens(mut self, value: bool) -> Self {
        self.check_hyphens = value;
        self
    }

    /// http://www.unicode.org/reports/tr46/#ToASCII
    pub fn to_ascii(self, domain: &str) -> Result<String, Errors> {
        let mut result = String::new();
        let mut first = true;
        let (domain, mut errors) = processing(domain, self);
        for label in domain.split('.') {
            if !first {
                result.push('.');
            }
            first = false;
            if label.is_ascii() {
                result.push_str(label);
            } else {
                match punycode::encode_str(label) {
                    Some(x) => {
                        result.push_str(PUNYCODE_PREFIX);
                        result.push_str(&x);
                    }
                    None => {
                        errors.punycode = true;
                    }
                }
            }
        }

        if self.verify_dns_length {
            let domain = if result.ends_with('.') {
                &result[..result.len() - 1]
            } else {
                &*result
            };
            if domain.is_empty() || domain.split('.').any(|label| label.is_empty()) {
                errors.too_short_for_dns = true;
            }
            if domain.len() > 253 || domain.split('.').any(|label| label.len() > 63) {
                errors.too_long_for_dns = true;
            }
        }

        Result::from(errors).map(|()| result)
    }

    /// http://www.unicode.org/reports/tr46/#ToUnicode
    pub fn to_unicode(self, domain: &str) -> (String, Result<(), Errors>) {
        let (domain, errors) = processing(domain, self);
        (domain, errors.into())
    }
}

fn is_bidi_domain(s: &str) -> bool {
    for c in s.chars() {
        if c.is_ascii_graphic() {
            continue;
        }
        match bidi_class(c) {
            BidiClass::R | BidiClass::AL | BidiClass::AN => return true,
            _ => {}
        }
    }
    false
}

/// Errors recorded during UTS #46 processing.
///
/// This is opaque for now, indicating what types of errors have been encountered at least once.
/// More details may be exposed in the future.
#[derive(Debug, Default)]
pub struct Errors {
    punycode: bool,
    // https://unicode.org/reports/tr46/#Validity_Criteria
    validity_criteria: bool,
    disallowed_by_std3_ascii_rules: bool,
    disallowed_mapped_in_std3: bool,
    disallowed_character: bool,
    too_long_for_dns: bool,
    too_short_for_dns: bool,
}

impl From<Errors> for Result<(), Errors> {
    fn from(e: Errors) -> Result<(), Errors> {
        let failed = e.punycode
            || e.validity_criteria
            || e.disallowed_by_std3_ascii_rules
            || e.disallowed_mapped_in_std3
            || e.disallowed_character
            || e.too_long_for_dns
            || e.too_short_for_dns;
        if !failed {
            Ok(())
        } else {
            Err(e)
        }
    }
}

impl StdError for Errors {}

impl fmt::Display for Errors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

#[cfg(test)]
mod tests {
    use super::{find_char, Mapping};

    #[test]
    fn mapping_fast_path() {
        assert!(matches!(find_char('-'), &Mapping::Valid));
        assert!(matches!(find_char('.'), &Mapping::Valid));
        for c in &['0', '1', '2', '3', '4', '5', '6', '7', '8', '9'] {
            assert!(matches!(find_char(*c), &Mapping::Valid));
        }
        for c in &[
            'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q',
            'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z',
        ] {
            assert!(matches!(find_char(*c), &Mapping::Valid));
        }
    }
}
