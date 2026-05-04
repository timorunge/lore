use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};

fn bench_sanitize_query(c: &mut Criterion) {
    let mut group = c.benchmark_group("sanitize_query");

    group.bench_function("short_simple", |b| {
        b.iter(|| lore::bench::sanitize_query(black_box("hello world")));
    });

    group.bench_function("with_field_prefix", |b| {
        b.iter(|| lore::bench::sanitize_query(black_box("topic:auth AND body:login")));
    });

    group.bench_function("with_brackets_quotes", |b| {
        b.iter(|| {
            lore::bench::sanitize_query(black_box(
                r#"[2024-01-01 TO 2024-12-31] "exact phrase" field:value"#,
            ))
        });
    });

    let long = "word ".repeat(200);
    group.bench_function("long_200_words", |b| {
        b.iter(|| lore::bench::sanitize_query(black_box(&long)));
    });

    group.bench_function("url_preserved", |b| {
        b.iter(|| {
            lore::bench::sanitize_query(black_box("https://example.com/path?q=search#fragment"))
        });
    });

    group.finish();
}

criterion_group!(benches, bench_sanitize_query);
criterion_main!(benches);
