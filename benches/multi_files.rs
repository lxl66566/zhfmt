//! Multi-file end-to-end benchmarks (parallel walker + IO + engine).
//! Requires the `bin` feature.

use std::{fs, path::Path};

use criterion::{Criterion, criterion_group, criterion_main};
use zhfmt::process::{RunOptions, process_paths};

fn make_corpus(dir: &Path, files: usize, paragraphs: usize) {
    let paragraph = "这是一段中文文本，mixed with English words 和数字 12345，还有 `code` 与 \
                     [link](https://example.com)。\n";
    let content = paragraph.repeat(paragraphs);
    for i in 0..files {
        let sub = dir.join(format!("dir{:02}", i % 16));
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join(format!("file{i:04}.md")), &content).unwrap();
    }
}

fn bench_multi_files(c: &mut Criterion) {
    let tmp = tempfile::TempDir::new().unwrap();
    // 256 files x ~16KB each.
    make_corpus(tmp.path(), 256, 256);

    let mut g = c.benchmark_group("multi_files");
    g.bench_function("check_256_files", |b| {
        b.iter(|| {
            process_paths(&[tmp.path().to_path_buf()], &RunOptions::check()).unwrap();
        });
    });
    g.finish();
}

criterion_group!(benches, bench_multi_files);
criterion_main!(benches);
