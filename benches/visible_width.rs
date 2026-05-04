use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};

fn bench_visible_width(c: &mut Criterion) {
    let mut group = c.benchmark_group("visible_width");

    group.bench_function("ascii_short", |b| {
        b.iter(|| lore::bench::visible_width(black_box("hello world")));
    });

    let ascii_long = "a".repeat(500);
    group.bench_function("ascii_500", |b| {
        b.iter(|| lore::bench::visible_width(black_box(&ascii_long)));
    });

    group.bench_function("ansi_colored", |b| {
        b.iter(|| {
            lore::bench::visible_width(black_box(
                "\x1b[1;31mERROR\x1b[0m: something \x1b[32mwent\x1b[0m wrong",
            ))
        });
    });

    let cjk = "\u{9a71}\u{52a8}\u{5668}".repeat(100);
    group.bench_function("cjk_300_chars", |b| {
        b.iter(|| lore::bench::visible_width(black_box(&cjk)));
    });

    let mixed = "Hello \u{4e16}\u{754c} World \x1b[1mBold\x1b[0m ".repeat(20);
    group.bench_function("mixed_ascii_cjk_ansi", |b| {
        b.iter(|| lore::bench::visible_width(black_box(&mixed)));
    });

    group.finish();
}

criterion_group!(benches, bench_visible_width);
criterion_main!(benches);
