use std::fmt::Write as _;
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};

fn bench_chunker(c: &mut Criterion) {
    let mut group = c.benchmark_group("chunker");

    let small = "# Introduction\n\nThis is a short document about Rust.\n\n\
                 ## Details\n\nRust is a systems programming language.\n";
    group.bench_function("small_200b", |b| {
        b.iter(|| lore::bench::chunk_markdown(black_box(small), 1024, 0));
    });

    let medium = {
        let mut s = String::with_capacity(50_000);
        for i in 0..100 {
            lore::w!(s, "## Section {i}\n\n");
            s.push_str(&"Lorem ipsum dolor sit amet. ".repeat(20));
            s.push('\n');
        }
        s
    };
    group.bench_function("medium_50kb", |b| {
        b.iter(|| lore::bench::chunk_markdown(black_box(&medium), 1024, 0));
    });

    let large = {
        let mut s = String::with_capacity(600_000);
        for i in 0..500 {
            if i % 50 == 0 {
                lore::w!(s, "# Chapter {}\n\n", i / 50);
            }
            lore::w!(s, "## Section {i}\n\n");
            s.push_str(&"The quick brown fox jumps over the lazy dog. ".repeat(25));
            s.push('\n');
        }
        s
    };
    group.bench_function("large_600kb", |b| {
        b.iter(|| lore::bench::chunk_markdown(black_box(&large), 1024, 0));
    });

    group.finish();
}

criterion_group!(benches, bench_chunker);
criterion_main!(benches);
