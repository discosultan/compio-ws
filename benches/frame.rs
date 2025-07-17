use std::hint::black_box;

use compio_ws::Frame;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use rand::{Rng, SeedableRng, rngs::SmallRng};

fn encode_control(c: &mut Criterion) {
    let mut group = c.benchmark_group("encode_control");
    group.sample_size(16384);
    for len in [0, 1, 31, 32, 33, 125] {
        group.bench_with_input(BenchmarkId::from_parameter(len), &len, |b, &len| {
            let mut rng = SmallRng::from_os_rng();
            let mut src = Vec::with_capacity(len);
            for i in 0..len {
                src.push(((i + 1) % usize::from(u8::MAX)) as u8);
            }
            let mut dst = Vec::with_capacity(src.len() + Frame::CONTROL_HEADER_LEN);
            b.iter(|| {
                let mask = rng.random::<u32>().to_ne_bytes();
                Frame::binary(&src).encode_control(&mut dst, mask);
            });
            black_box(dst);
        });
    }
}

fn encode(c: &mut Criterion) {
    let mut group = c.benchmark_group("encode");
    group.sample_size(4096);
    for len in [1, 16, 125, 126, 65535, 65536] {
        group.bench_with_input(BenchmarkId::from_parameter(len), &len, |b, &len| {
            let mut rng = SmallRng::from_os_rng();
            let mut src = Vec::with_capacity(len + 14);
            for i in 0..len {
                src.push(((i + 1) % usize::from(u8::MAX)) as u8);
            }
            let mut dst = Vec::with_capacity(src.len() + Frame::MAX_HEADER_LEN);
            b.iter(|| {
                let mask = rng.random::<u32>().to_ne_bytes();
                Frame::binary(&src).encode(&mut dst, mask);
            });
            black_box(dst);
        });
    }
}

fn validate_utf8(c: &mut Criterion) {
    let mut group = c.benchmark_group("validate_utf8");
    group.sample_size(4096);
    for len in [1, 16, 128, 256, 1024, 65535, 65536] {
        group.bench_with_input(BenchmarkId::from_parameter(len), &len, |b, &len| {
            let mut data = Vec::with_capacity(len);
            for i in 0..len {
                let ascii_code = (i % 95) + 32;
                data.push(ascii_code as u8);
            }
            b.iter(|| {
                assert!(Frame::validate_utf8(&data).is_some());
            });
        });
    }
}

criterion_group!(benches, encode_control, encode, validate_utf8);
criterion_main!(benches);
