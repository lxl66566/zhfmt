//! Single-thread engine throughput benchmarks.
//!
//! Not meant to be run on weak machines; use CI or a decent box. Targets
//! multi-GB/s scanning on ASCII-heavy inputs (SWAR fast path).
// Bench closures return the formatted buffer to keep `black_box` effective.
#![allow(clippy::semicolon_if_nothing_returned)]

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

fn bench_pure_ascii(c: &mut Criterion) {
    let input = "lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(16 * 1024);
    let mut g = c.benchmark_group("pure_ascii");
    g.throughput(Throughput::Bytes(input.len() as u64));
    g.bench_function("format", |b| {
        b.iter(|| zhfmt::format(std::hint::black_box(input.as_bytes())))
    });
    g.finish();
}

fn bench_mixed_document(c: &mut Criterion) {
    let paragraph = "这是一段中文文本，mixed with some English words like performance 和 \
                     correctness，还有数字 12345 以及一些 `inline_code` 和 [link](https://example.com)。\n";
    let input = paragraph.repeat(4 * 1024);
    let mut g = c.benchmark_group("mixed_document");
    g.throughput(Throughput::Bytes(input.len() as u64));
    g.bench_function("format", |b| {
        b.iter(|| zhfmt::format(std::hint::black_box(input.as_bytes())))
    });
    g.finish();
}

fn bench_pure_cjk(c: &mut Criterion) {
    let input = "这是一段纯粹的中文文本，没有任何混排的内容。".repeat(16 * 1024);
    let mut g = c.benchmark_group("pure_cjk");
    g.throughput(Throughput::Bytes(input.len() as u64));
    g.bench_function("format", |b| {
        b.iter(|| zhfmt::format(std::hint::black_box(input.as_bytes())))
    });
    g.finish();
}

fn bench_already_formatted(c: &mut Criterion) {
    let paragraph = "这是一段 已经 格式化好的 文本，with spaces everywhere 12345 needed。\n";
    let input = paragraph.repeat(4 * 1024);
    let mut g = c.benchmark_group("already_formatted");
    g.throughput(Throughput::Bytes(input.len() as u64));
    g.bench_function("format", |b| {
        b.iter(|| zhfmt::format(std::hint::black_box(input.as_bytes())))
    });
    g.finish();
}

fn bench_sizes(c: &mut Criterion) {
    let paragraph = "中文 mixed English 文本 with 数字 123。\n";
    let mut g = c.benchmark_group("sizes");
    for kb in [1usize, 16, 256, 4096] {
        let input = paragraph.repeat(kb * 1024 / paragraph.len() + 1);
        g.throughput(Throughput::Bytes(input.len() as u64));
        g.bench_with_input(
            BenchmarkId::from_parameter(format!("{kb}KB")),
            &input,
            |b, input| {
                b.iter(|| zhfmt::format(std::hint::black_box(input.as_bytes())));
            },
        );
    }
    g.finish();
}

criterion_group!(
    benches,
    bench_pure_ascii,
    bench_mixed_document,
    bench_pure_cjk,
    bench_already_formatted,
    bench_sizes
);
criterion_main!(benches);
