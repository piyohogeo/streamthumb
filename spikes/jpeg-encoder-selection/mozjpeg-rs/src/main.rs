use std::hint::black_box;
use std::time::Instant;

fn main() {
    let iterations = std::env::args()
        .nth(1)
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(100);
    let first = mozjpeg_rs_spike::encode().expect("the spike image must encode");
    let start = Instant::now();
    let mut output_bytes = 0_usize;

    for _ in 0..iterations {
        output_bytes = black_box(
            mozjpeg_rs_spike::encode()
                .expect("the spike image must encode")
                .len(),
        );
    }

    let elapsed = start.elapsed();
    println!("encoder=mozjpeg-rs");
    println!("iterations={iterations}");
    println!("output_bytes={output_bytes}");
    println!("first_output_bytes={}", first.len());
    println!("total_micros={}", elapsed.as_micros());
    println!(
        "mean_micros={:.2}",
        elapsed.as_secs_f64() * 1_000_000.0 / f64::from(iterations)
    );
}
