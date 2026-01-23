use criterion::{criterion_group, criterion_main, Criterion};

fn http_parser_benchmarks(_c: &mut Criterion) {
    // TODO: Add HTTP parser benchmarks
}

criterion_group!(benches, http_parser_benchmarks);
criterion_main!(benches);
