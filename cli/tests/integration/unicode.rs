use std::fs;

use lore::fmt::visible_width;
use tempfile::TempDir;

use crate::helpers::*;

const FAMILY: &str = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";

fn unicode_fixture(language: &str) -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let docs = dir.path().join("docs");
    fs::create_dir_all(&docs).unwrap();
    let files = [
        (
            "zh.md",
            "# 机器学习入门\n\n机器学习是人工智能的一个分支。深度学习使用神经网络。\n".to_owned(),
        ),
        (
            "ja.md",
            "# 東京の天気予報\n\n今日の東京は晴れです。カタカナのテスト。\n".to_owned(),
        ),
        (
            "ko.md",
            "# 한국어 문서\n\n서울의 날씨는 맑습니다. 데이터베이스 검색.\n".to_owned(),
        ),
        (
            "ar.md",
            "# مرحبا بالعالم\n\nهذا مستند تجريبي باللغة العربية.\n".to_owned(),
        ),
        (
            "emoji.md",
            format!(
                "---\ntitle: Party {FAMILY} 🎉\n---\n# Release 🚀\n\nShipped the rocket 🚀. Flags 🇯🇵. Café naïve.\n"
            ),
        ),
        (
            "ascii.md",
            "# Plain\n\nJust plain ascii text here.\n".to_owned(),
        ),
    ];
    for (name, body) in files {
        fs::write(docs.join(name), body).unwrap();
    }
    let config = format!(
        "name: Unicode\nsources:\n  - path: {}\n    glob: \"**/*.md\"\nstore:\n  path: idx\n  language: {language}\n",
        docs.display()
    );
    let config_path = run_ingest(dir.path(), &config);
    (dir, config_path)
}

fn hit_sources(config_path: &std::path::Path, query: &str) -> Vec<String> {
    let json = collect_json(
        lore()
            .args(["search", "--json", "--config"])
            .arg(config_path)
            .arg(query),
    );
    let hits = json
        .as_array()
        .cloned()
        .or_else(|| {
            json.as_object()
                .and_then(|o| o.values().find_map(|v| v.as_array().cloned()))
        })
        .unwrap_or_default();
    hits.iter()
        .filter_map(|h| h["source"].as_str().map(str::to_owned))
        .collect()
}

#[test]
fn search_finds_every_script() {
    for language in ["english", "none"] {
        let (_dir, config_path) = unicode_fixture(language);
        let cases = [
            ("机器学习", "zh.md"),
            ("神经网络", "zh.md"),
            ("学习", "zh.md"),
            ("東京の天気", "ja.md"),
            ("カタカナ", "ja.md"),
            ("서울의", "ko.md"),
            ("데이터베이스", "ko.md"),
            ("العربية", "ar.md"),
            ("🚀", "emoji.md"),
            ("café", "emoji.md"),
            ("rocket", "emoji.md"),
        ];
        for (query, file) in cases {
            let hits = hit_sources(&config_path, query);
            assert!(
                hits.first().is_some_and(|s| s.ends_with(file)),
                "[{language}] {query:?} -> {hits:?}, expected {file}"
            );
        }
    }
}

#[test]
fn table_columns_align_with_wide_text() {
    let (_dir, config_path) = unicode_fixture("none");
    let out = collect_stdout(
        lore()
            .env("COLUMNS", "120")
            .env("NO_COLOR", "1")
            .args(["docs", "--config"])
            .arg(&config_path),
    );
    // Every row's "words" column must end at the same display column.
    let ends: Vec<usize> = out
        .lines()
        .filter(|l| l.contains(" words"))
        .map(|l| {
            let at = l.find(" words").unwrap();
            visible_width(&l[..at])
        })
        .collect();
    assert_eq!(ends.len(), 6, "{out}");
    assert!(
        ends.windows(2).all(|w| w[0] == w[1]),
        "misaligned: {ends:?}\n{out}"
    );
}

#[test]
fn read_and_info_render_wide_text() {
    let (_dir, config_path) = unicode_fixture("none");
    for args in [
        vec!["info"],
        vec!["topics"],
        vec!["search", "机器学习"],
        vec!["search", "🚀"],
    ] {
        let out = collect_stdout(
            lore()
                .env("COLUMNS", "40")
                .env("NO_COLOR", "1")
                .args(&args)
                .arg("--config")
                .arg(&config_path),
        );
        for line in out.lines() {
            assert!(
                visible_width(line) <= 40,
                "{args:?}: line wider than terminal: {line:?}"
            );
        }
    }
}
