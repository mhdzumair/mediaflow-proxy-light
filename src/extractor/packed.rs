//! Port of Dean Edward's P.A.C.K.E.R JavaScript unpacker.
//!
//! Reference: mediaflow-proxy/mediaflow_proxy/utils/packed.py

use regex::Regex;

/// Returns `true` if `source` contains a P.A.C.K.E.R. payload.
pub fn is_packed(source: &str) -> bool {
    source.contains("eval(function(p,a,c,k,e,d)") || source.contains("eval(function(p,a,c,k,e,r)")
}

/// Attempt to unpack P.A.C.K.E.R. obfuscated JavaScript.
/// Returns `None` if unpacking fails.
pub fn unpack_packed_js(source: &str) -> Option<String> {
    // Extract (payload, symtab_str, radix, count).
    let (payload, symtab, radix, _count) = filter_args(source)?;

    let unpacked = apply_symtab(&payload, &symtab, radix);
    Some(replace_strings(&unpacked))
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn filter_args(source: &str) -> Option<(String, Vec<String>, u32, usize)> {
    let juicers: &[&str] = &[
        r"}\('(.*)', *(\d+|\[\]), *(\d+), *'(.*)'\.split\('\|'\), *(\d+), *(.*)\)\)",
        r"}\('(.*)', *(\d+|\[\]), *(\d+), *'(.*)'\.split\('\|'\)",
    ];

    for juicer in juicers {
        let re = Regex::new(juicer).ok()?;
        if let Some(cap) = re.captures(source) {
            let payload = cap.get(1)?.as_str().to_string();
            let radix_str = cap.get(2)?.as_str();
            let radix: u32 = if radix_str == "[]" {
                62
            } else {
                radix_str.parse().ok()?
            };
            let count: usize = cap.get(3)?.as_str().parse().ok()?;
            let symtab_str = cap.get(4)?.as_str();
            let symtab: Vec<String> = symtab_str.split('|').map(String::from).collect();

            return Some((payload, symtab, radix, count));
        }
    }
    None
}

fn apply_symtab(payload: &str, symtab: &[String], radix: u32) -> String {
    let payload = payload.replace("\\\\", "\\").replace("\\'", "'");
    let re = Regex::new(r"\b(\w+)\b").unwrap();
    re.replace_all(&payload, |caps: &regex::Captures| {
        let word = &caps[1];
        let idx = unbase(word, radix) as usize;
        if idx < symtab.len() && !symtab[idx].is_empty() {
            symtab[idx].clone()
        } else {
            word.to_string()
        }
    })
    .into_owned()
}

fn replace_strings(source: &str) -> String {
    let re = Regex::new(r#"var\s+(_\w+)=\["(.*?)"\];"#).unwrap();
    if let Some(cap) = re.captures(source) {
        let varname = &cap[1];
        let strings: Vec<&str> = cap[2].split("\",\"").collect();
        let escaped_varname = regex::escape(varname);
        let index_re = Regex::new(&format!(r"{escaped_varname}\[(\d+)\]")).unwrap();
        return index_re
            .replace_all(source, |c: &regex::Captures| {
                let idx: usize = c[1].parse().unwrap_or(0);
                strings.get(idx).copied().unwrap_or("").to_string()
            })
            .into_owned();
    }
    source.to_string()
}

/// Convert a base-N string to a number (base 2–62).
fn unbase(s: &str, base: u32) -> u64 {
    const CHARS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut result: u64 = 0;
    for byte in s.bytes() {
        let digit = CHARS.iter().position(|&c| c == byte).unwrap_or(0) as u64;
        result = result * (base as u64) + digit;
    }
    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_packed() {
        assert!(is_packed("eval(function(p,a,c,k,e,d){})"));
        assert!(!is_packed("some other js"));
    }

    #[test]
    fn test_unbase() {
        assert_eq!(unbase("10", 10), 10);
        assert_eq!(unbase("a", 16), 10);
        assert_eq!(unbase("z", 36), 35);
    }
}
