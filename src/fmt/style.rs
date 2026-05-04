//! Lightweight ANSI color support.

use std::fmt;

/// Wraps text in ANSI escape codes when color is enabled for the target fd.
#[derive(Clone, Copy)]
pub struct Painter(bool);

impl Painter {
    /// Create a Painter with color enabled or disabled.
    pub fn new(color: bool) -> Self {
        Self(color)
    }

    /// Whether ANSI codes will be emitted.
    pub fn enabled(self) -> bool {
        self.0
    }

    /// Wrap text in bold ANSI style.
    pub fn bold(self, s: &str) -> Styled<'_> {
        Styled {
            s,
            code: "1",
            on: self.0,
        }
    }

    /// Wrap text in bold yellow ANSI style (search highlights).
    pub fn bold_yellow(self, s: &str) -> Styled<'_> {
        Styled {
            s,
            code: "1;33",
            on: self.0,
        }
    }

    /// Wrap text in dim ANSI style.
    pub fn dim(self, s: &str) -> Styled<'_> {
        Styled {
            s,
            code: "2",
            on: self.0,
        }
    }

    /// Wrap text in red ANSI style.
    pub fn red(self, s: &str) -> Styled<'_> {
        Styled {
            s,
            code: "31",
            on: self.0,
        }
    }

    /// Wrap text in green ANSI style.
    pub fn green(self, s: &str) -> Styled<'_> {
        Styled {
            s,
            code: "32",
            on: self.0,
        }
    }

    /// Wrap text in blue ANSI style.
    pub fn blue(self, s: &str) -> Styled<'_> {
        Styled {
            s,
            code: "34",
            on: self.0,
        }
    }

    /// Wrap text in yellow ANSI style.
    pub fn yellow(self, s: &str) -> Styled<'_> {
        Styled {
            s,
            code: "33",
            on: self.0,
        }
    }

    /// Wrap text in purple (magenta) ANSI style.
    pub fn purple(self, s: &str) -> Styled<'_> {
        Styled {
            s,
            code: "35",
            on: self.0,
        }
    }
}

/// A styled string reference that renders ANSI codes via `Display`.
/// Zero-allocation when used via `Display`; `.to_string()` allocates.
pub struct Styled<'a> {
    s: &'a str,
    code: &'static str,
    on: bool,
}

impl fmt::Display for Styled<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.on {
            write!(f, "\x1b[{}m{}\x1b[0m", self.code, self.s)
        } else {
            f.write_str(self.s)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::type_complexity)]
    fn styled_display() {
        let cases: &[(&str, &str, fn(Painter, &str) -> Styled<'_>)] = &[
            ("1", "bold", |p, s| p.bold(s)),
            ("1;33", "bold_yellow", |p, s| p.bold_yellow(s)),
            ("2", "dim", |p, s| p.dim(s)),
            ("31", "red", |p, s| p.red(s)),
            ("32", "green", |p, s| p.green(s)),
            ("33", "yellow", |p, s| p.yellow(s)),
            ("34", "blue", |p, s| p.blue(s)),
            ("35", "purple", |p, s| p.purple(s)),
        ];
        for &(code, label, method) in cases {
            let on = Painter(true);
            assert_eq!(
                method(on, "x").to_string(),
                format!("\x1b[{code}mx\x1b[0m"),
                "{label} with color on"
            );
            let off = Painter(false);
            assert_eq!(method(off, "x").to_string(), "x", "{label} with color off");
        }
    }
}
