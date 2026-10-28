//! Word tokenizer that also handles scripts written without spaces.
//!
//! Splits like tantivy's `SimpleTokenizer` (alphanumeric runs), with two
//! additions: runs of Han, Hiragana, and Katakana are emitted as overlapping
//! bigrams, the usual dictionary-free approach for CJK full-text search, and
//! emoji pictographs become single-character tokens instead of being dropped.
//! Hangul is space-delimited, so it stays on the word path.

use tantivy::tokenizer::{Token, TokenStream, Tokenizer};

/// Tokenizer producing words, CJK bigrams, and emoji tokens.
#[derive(Clone, Default)]
pub(crate) struct LoreTokenizer {
    token: Token,
}

/// Token stream for `LoreTokenizer`. Tokens are precomputed as byte ranges.
pub(crate) struct LoreTokenStream<'a> {
    text: &'a str,
    spans: std::vec::IntoIter<(usize, usize)>,
    token: &'a mut Token,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Word,
    Cjk,
    Emoji,
    Other,
}

impl Tokenizer for LoreTokenizer {
    type TokenStream<'a> = LoreTokenStream<'a>;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> LoreTokenStream<'a> {
        self.token.reset();
        LoreTokenStream {
            text,
            spans: spans(text).into_iter(),
            token: &mut self.token,
        }
    }
}

impl TokenStream for LoreTokenStream<'_> {
    fn advance(&mut self) -> bool {
        let Some((from, to)) = self.spans.next() else {
            return false;
        };
        self.token.text.clear();
        self.token.text.push_str(&self.text[from..to]);
        self.token.offset_from = from;
        self.token.offset_to = to;
        self.token.position = self.token.position.wrapping_add(1);
        true
    }

    fn token(&self) -> &Token {
        self.token
    }

    fn token_mut(&mut self) -> &mut Token {
        self.token
    }
}

fn classify(c: char) -> Class {
    if is_cjk(c) {
        Class::Cjk
    } else if c.is_alphanumeric() {
        Class::Word
    } else if is_emoji(c) {
        Class::Emoji
    } else {
        Class::Other
    }
}

/// Han ideographs, kana, and CJK compatibility forms.
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF | 0x31F0..=0x31FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF
        | 0xF900..=0xFAFF | 0xFF66..=0xFF9F | 0x20000..=0x3FFFD)
}

/// Emoji pictographs. Modifiers, joiners, and variation selectors are not
/// tokens of their own; regional indicators are, so a flag yields two.
fn is_emoji(c: char) -> bool {
    matches!(c as u32,
        0x2600..=0x27BF | 0x1F000..=0x1F3FA | 0x1F400..=0x1FAFF)
}

/// Rewrite unquoted CJK runs in a query as space-separated bigrams.
///
/// The query parser turns a word that tokenizes into several terms into a
/// phrase query, which fails on stores indexed without positions (the
/// default). Separate terms give a plain boolean query instead. Inside
/// quotes the bigrams stay adjacent, so a positional store still matches
/// them as a phrase.
pub(crate) fn split_cjk_terms(query: &str) -> String {
    let mut out = String::with_capacity(query.len() * 2);
    let mut run: Vec<char> = Vec::new();
    for c in query.chars() {
        if is_cjk(c) {
            run.push(c);
            continue;
        }
        push_bigrams(&run, &mut out);
        run.clear();
        out.push(c);
    }
    push_bigrams(&run, &mut out);
    out.trim().to_owned()
}

/// Append `run` as standalone terms, padded so it never fuses with a
/// neighbouring Latin word into one multi-term parser word.
fn push_bigrams(run: &[char], out: &mut String) {
    if run.is_empty() {
        return;
    }
    out.push(' ');
    if run.len() <= 2 {
        out.extend(run);
    } else {
        for (i, pair) in run.windows(2).enumerate() {
            if i > 0 {
                out.push(' ');
            }
            out.extend(pair);
        }
    }
    out.push(' ');
}

/// Byte ranges of all tokens in `text`, in order.
fn spans(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut run: Vec<(usize, char)> = Vec::new();
    let mut run_class = Class::Other;
    for (i, c) in text.char_indices() {
        let class = classify(c);
        if class != run_class || class == Class::Emoji {
            flush(&run, run_class, &mut out);
            run.clear();
            run_class = class;
        }
        if class != Class::Other {
            run.push((i, c));
        }
    }
    flush(&run, run_class, &mut out);
    out
}

fn flush(run: &[(usize, char)], class: Class, out: &mut Vec<(usize, usize)>) {
    let (Some(&(start, _)), Some(&(last, last_c))) = (run.first(), run.last()) else {
        return;
    };
    let end = last + last_c.len_utf8();
    match class {
        Class::Word | Class::Emoji => out.push((start, end)),
        Class::Cjk if run.len() == 1 => out.push((start, end)),
        Class::Cjk => {
            for pair in run.windows(2) {
                let (from, _) = pair[0];
                let (second, c) = pair[1];
                out.push((from, second + c.len_utf8()));
            }
        }
        Class::Other => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(text: &str) -> Vec<String> {
        let mut t = LoreTokenizer::default();
        let mut stream = t.token_stream(text);
        let mut out = Vec::new();
        while stream.advance() {
            out.push(stream.token().text.clone());
        }
        out
    }

    #[test]
    fn tokenize_table() {
        let cases: &[(&str, &[&str])] = &[
            (
                "Hello, happy tax-payer!",
                &["Hello", "happy", "tax", "payer"],
            ),
            ("机器学习", &["机器", "器学", "学习"]),
            ("学", &["学"]),
            ("東京の天気", &["東京", "京の", "の天", "天気"]),
            ("カタカナ", &["カタ", "タカ", "カナ"]),
            ("서울의 날씨는", &["서울의", "날씨는"]),
            ("Rust与机器学习", &["Rust", "与机", "机器", "器学", "学习"]),
            ("ship 🚀🎉 now", &["ship", "🚀", "🎉", "now"]),
            ("👨\u{200d}👩\u{200d}👧", &["👨", "👩", "👧"]),
            ("👍🏽", &["👍"]),
            ("café naïve", &["café", "naïve"]),
            ("مرحبا بالعالم", &["مرحبا", "بالعالم"]),
            ("", &[]),
            ("...  --", &[]),
        ];
        for (text, expected) in cases {
            assert_eq!(&tokens(text), expected, "{text}");
        }
    }

    #[test]
    fn split_cjk_terms_table() {
        let cases = [
            ("rust", "rust"),
            ("学习", "学习"),
            ("机器学习", "机器 器学 学习"),
            ("rust机器学习", "rust 机器 器学 学习"),
            ("东京都", "东京 京都"),
            ("서울의 날씨는", "서울의 날씨는"),
        ];
        for (q, expected) in cases {
            let got = split_cjk_terms(q);
            let words: Vec<_> = got.split_whitespace().collect();
            assert_eq!(words.join(" "), expected, "{q}");
        }
    }

    #[test]
    fn offsets_and_positions_are_consistent() {
        let text = "Rust与机器学习 🚀";
        let mut t = LoreTokenizer::default();
        let mut stream = t.token_stream(text);
        let mut pos = Vec::new();
        while stream.advance() {
            let tok = stream.token();
            assert_eq!(&text[tok.offset_from..tok.offset_to], tok.text);
            pos.push(tok.position);
        }
        assert_eq!(pos, (0..pos.len()).collect::<Vec<_>>());
    }
}
