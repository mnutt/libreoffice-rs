//! Common Excel display formats for cached preview values (no formula evaluation).
//! Locale-specific calendars, fractions and conditional format sections are not implemented.

pub fn builtin(id: u32) -> &'static str {
    match id {
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        5 | 6 => "\"$\"#,##0;[Red](\"$\"#,##0)",
        7 | 8 => "\"$\"#,##0.00;[Red](\"$\"#,##0.00)",
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        12 => "# ?/?",
        13 => "# ??/??",
        14 => "m/d/yy",
        15 => "d-mmm-yy",
        16 => "d-mmm",
        17 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 => "h:mm",
        21 => "h:mm:ss",
        22 => "m/d/yy h:mm",
        37 | 38 => "#,##0;(#,##0)",
        39 | 40 => "#,##0.00;(#,##0.00)",
        45 => "mm:ss",
        46 => "[h]:mm:ss",
        47 => "mmss.0",
        48 => "##0.0E+0",
        49 => "@",
        _ => "General",
    }
}

#[derive(Debug, Clone)]
enum Token {
    Literal(String),
    Pattern(char),
    Elapsed(char, usize),
}

fn sections(code: &str) -> Vec<String> {
    let mut sections = vec![String::new()];
    let mut quoted = false;
    let mut escaped = false;
    for c in code.chars() {
        if c == ';' && !quoted && !escaped {
            sections.push(String::new());
        } else {
            sections.last_mut().unwrap().push(c);
        }
        if c == '"' && !escaped {
            quoted = !quoted;
        }
        if escaped {
            escaped = false;
        } else {
            escaped = c == '\\';
        }
    }
    sections
}

fn tokenize(code: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut chars = code.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                let literal: String = chars.by_ref().take_while(|c| *c != '"').collect();
                out.push(Token::Literal(literal));
            }
            '\\' => {
                if let Some(c) = chars.next() {
                    out.push(Token::Literal(c.to_string()));
                }
            }
            '_' | '*' => {
                chars.next();
            }
            '[' => {
                let text: String = chars.by_ref().take_while(|c| *c != ']').collect();
                let lower = text.to_ascii_lowercase();
                if matches!(lower.as_str(), "h" | "hh" | "m" | "mm" | "s" | "ss") {
                    out.push(Token::Elapsed(lower.chars().next().unwrap(), lower.len()));
                } else if let Some(currency) = text.strip_prefix('$') {
                    out.push(Token::Literal(
                        currency.split('-').next().unwrap_or("").into(),
                    ));
                }
            }
            _ => out.push(Token::Pattern(c)),
        }
    }
    out
}

pub fn number(value: f64, code: &str, date_1904: bool) -> String {
    if !value.is_finite() || code.eq_ignore_ascii_case("General") || code == "@" || code.is_empty()
    {
        return value.to_string();
    }
    // Do not choose a sign-based section when the format uses value conditions.
    if ["[<", "[>", "[="]
        .iter()
        .any(|prefix| code.contains(prefix))
    {
        return value.to_string();
    }
    let parts = sections(code);
    let explicit_negative = value < 0.0 && parts.len() > 1;
    let section = if explicit_negative {
        &parts[1]
    } else if value == 0.0 && parts.len() > 2 {
        &parts[2]
    } else {
        &parts[0]
    };
    let tokens = tokenize(section);
    if tokens.iter().any(|t| {
        matches!(
            t,
            Token::Pattern('y' | 'Y' | 'd' | 'D' | 'h' | 'H' | 's' | 'S') | Token::Elapsed(_, _)
        )
    }) {
        return date(value, &tokens, date_1904).unwrap_or_else(|| value.to_string());
    }
    let positions: Vec<_> = tokens
        .iter()
        .enumerate()
        .filter_map(|(i, t)| matches!(t, Token::Pattern('0' | '#' | '?')).then_some(i))
        .collect();
    let Some(&start) = positions.first() else {
        return literals(&tokens);
    };
    let end = *positions.last().unwrap();
    let pattern = literals(&tokens[start..=end]);
    // Fractions need a rational approximation rather than decimal rounding.
    if pattern.contains('/') {
        return value.to_string();
    }
    let percents = tokens
        .iter()
        .filter(|t| matches!(t, Token::Pattern('%')))
        .count();
    let mut scaled = value.abs() * 100_f64.powi(percents.min(4) as i32);
    let trailing_commas = tokens[end + 1..]
        .iter()
        .take_while(|t| matches!(t, Token::Pattern(',')))
        .count();
    scaled /= 1000_f64.powi(trailing_commas.min(4) as i32);
    if !scaled.is_finite() {
        return value.to_string();
    }
    let (mantissa, exponent) = pattern
        .split_once(['E', 'e'])
        .map(|(a, b)| (a, Some(b)))
        .unwrap_or((&pattern, None));
    let decimal = mantissa.split_once('.').map(|(_, b)| b).unwrap_or("");
    let places = decimal
        .chars()
        .filter(|c| matches!(c, '0' | '#' | '?'))
        .count()
        .min(15);
    let mandatory = decimal.chars().filter(|c| *c == '0').count().min(15);
    let formatted = if let Some(exponent) = exponent {
        let scientific = format!("{scaled:.places$e}");
        let (base, exp) = scientific.split_once('e').unwrap();
        let exp: i32 = exp.parse().unwrap_or(0);
        let width = exponent.chars().filter(|c| *c == '0').count().min(6);
        format!(
            "{base}E{}{:0width$}",
            if exp < 0 {
                "-"
            } else if exponent.starts_with('+') {
                "+"
            } else {
                ""
            },
            exp.unsigned_abs()
        )
    } else {
        let factor = 10_f64.powi(places as i32);
        let rounded = if (scaled * factor).is_finite() {
            (scaled * factor).round() / factor
        } else {
            scaled
        };
        let mut digits = format!("{rounded:.places$}");
        if places > mandatory {
            for _ in mandatory..places {
                if digits.ends_with('0') {
                    digits.pop();
                } else {
                    break;
                }
            }
            if digits.ends_with('.') {
                digits.pop();
            }
        }
        let (integer, fraction) = digits.split_once('.').unwrap_or((&digits, ""));
        let integer_pattern = mantissa.split('.').next().unwrap_or("");
        let min_digits = integer_pattern
            .chars()
            .filter(|c| *c == '0')
            .count()
            .min(30);
        let mut integer = format!("{:0>width$}", integer, width = min_digits);
        if integer_pattern.contains(',') {
            integer = integer
                .chars()
                .rev()
                .enumerate()
                .flat_map(|(i, c)| {
                    if i > 0 && i % 3 == 0 {
                        vec![',', c]
                    } else {
                        vec![c]
                    }
                })
                .collect::<String>()
                .chars()
                .rev()
                .collect();
        }
        if fraction.is_empty() {
            integer
        } else {
            format!("{integer}.{fraction}")
        }
    };
    let prefix = literals(&tokens[..start]);
    let suffix = literals(&tokens[end + 1 + trailing_commas..]);
    let sign = if value < 0.0 && !explicit_negative {
        "-"
    } else {
        ""
    };
    format!("{sign}{prefix}{formatted}{suffix}")
}

fn literals(tokens: &[Token]) -> String {
    tokens
        .iter()
        .map(|t| match t {
            Token::Literal(s) => s.clone(),
            Token::Pattern(c) => c.to_string(),
            Token::Elapsed(c, n) => c.to_string().repeat(*n),
        })
        .collect()
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    y += i64::from(m <= 2);
    (y, m as u32, d as u32)
}

fn date(value: f64, tokens: &[Token], date_1904: bool) -> Option<String> {
    if !(0.0..=2_958_465.99999).contains(&value) {
        return None;
    }
    let seconds = (value * 86400.0).round() as i64;
    let serial = seconds / 86400;
    let days = if date_1904 {
        serial - 24107
    } else {
        serial - 25568 - i64::from(serial >= 60)
    };
    let (year, month, day) = if !date_1904 && serial == 60 {
        (1900, 2, 29)
    } else {
        civil_from_days(days)
    };
    let hour = seconds / 3600 % 24;
    let minute = seconds / 60 % 60;
    let second = seconds % 60;
    let months = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let weekdays = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    let text = literals(tokens).to_ascii_lowercase();
    let ampm = text.contains("am/pm");
    let mut result = String::new();
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            Token::Literal(s) => {
                result.push_str(s);
                i += 1;
            }
            Token::Elapsed(c, n) => {
                let v = match c {
                    'h' => seconds / 3600,
                    'm' => seconds / 60,
                    _ => seconds,
                };
                result.push_str(&format!("{v:0width$}", width = *n));
                i += 1;
            }
            Token::Pattern(c) => {
                let lower = c.to_ascii_lowercase();
                if lower == 'a'
                    && i + 5 <= tokens.len()
                    && literals(&tokens[i..i + 5]).eq_ignore_ascii_case("AM/PM")
                {
                    result.push_str(if hour < 12 { "AM" } else { "PM" });
                    i += 5;
                    continue;
                }
                let mut end = i + 1;
                while end < tokens.len()
                    && matches!(tokens[end],Token::Pattern(next) if next.to_ascii_lowercase()==lower)
                {
                    end += 1;
                }
                let n = end - i;
                let previous = tokens[..i].iter().rev().find_map(|t| match t {
                    Token::Pattern(c) if c.is_ascii_alphabetic() => Some(c.to_ascii_lowercase()),
                    Token::Elapsed(c, _) => Some(*c),
                    _ => None,
                });
                let next = tokens[end..].iter().find_map(|t| match t {
                    Token::Pattern(c) if c.is_ascii_alphabetic() => Some(c.to_ascii_lowercase()),
                    _ => None,
                });
                let v = match lower {
                    'y' => {
                        if n == 2 {
                            format!("{:02}", year % 100)
                        } else {
                            format!("{year:04}")
                        }
                    }
                    'm' if previous == Some('h') || next == Some('s') => {
                        if n == 1 {
                            minute.to_string()
                        } else {
                            format!("{minute:02}")
                        }
                    }
                    'm' => match n {
                        1 => month.to_string(),
                        2 => format!("{month:02}"),
                        3 => months[month as usize - 1][..3].into(),
                        4 => months[month as usize - 1].into(),
                        _ => months[month as usize - 1][..1].into(),
                    },
                    'd' => match n {
                        1 => day.to_string(),
                        2 => format!("{day:02}"),
                        3 => weekdays[(days + 4).rem_euclid(7) as usize][..3].into(),
                        _ => weekdays[(days + 4).rem_euclid(7) as usize].into(),
                    },
                    'h' => {
                        let h = if ampm { (hour + 11) % 12 + 1 } else { hour };
                        if n == 1 {
                            h.to_string()
                        } else {
                            format!("{h:02}")
                        }
                    }
                    's' => {
                        if n == 1 {
                            second.to_string()
                        } else {
                            format!("{second:02}")
                        }
                    }
                    _ => c.to_string().repeat(n),
                };
                result.push_str(&v);
                i = end;
            }
        }
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn numeric_formats_preserve_precision_and_literals() {
        for (value, code, expected) in [
            (12.5, "\"$\"#,##0.00", "$12.50"),
            (0.125, "0.0%", "12.5%"),
            (12345.678, "#,##0.00", "12,345.68"),
            (-12.5, "0.00;[Red](0.00)", "(12.50)"),
            (10.5, "0.###", "10.5"),
            (1000.0, "0.00E+00", "1.00E+03"),
            (3.0, "0000", "0003"),
            (12000.0, "0,", "12"),
            (0.0, "0;-0;\"zero\"", "zero"),
            (2.5, "0", "3"),
            (-2.5, "0", "-3"),
            (-12.5, "\"$\"0.00", "-$12.50"),
        ] {
            assert_eq!(number(value, code, false), expected, "{code}");
        }
    }
    #[test]
    fn overflowing_display_scaling_falls_back_without_panicking() {
        assert_eq!(number(1e308, "0.00E+00%", false), 1e308_f64.to_string());
    }

    #[test]
    fn dates_times_epochs_and_leap_bug() {
        for (value, code, epoch, expected) in [
            (39045.0, "yyyy/mm/dd", false, "2006/11/24"),
            (1.0, "yyyy-mm-dd", false, "1900-01-01"),
            (60.0, "yyyy-mm-dd", false, "1900-02-29"),
            (61.0, "yyyy-mm-dd", false, "1900-03-01"),
            (0.0, "yyyy-mm-dd", true, "1904-01-01"),
            (0.75, "h:mm AM/PM", false, "6:00 PM"),
            (1.5, "[hh]:mm:ss", false, "36:00:00"),
        ] {
            assert_eq!(number(value, code, epoch), expected, "{code}");
        }
    }
}
