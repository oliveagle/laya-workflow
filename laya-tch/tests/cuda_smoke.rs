//! GPU smoke test: exercises a tiny matmul on a CUDA device to prove the
//! tch/libtorch CUDA stack actually executes. Skipped (no-op) when the linked
//! libtorch has no CUDA support, so CPU-only builds and CI are unaffected.
use laya_tch::preload_torch_cuda;

#[test]
fn cuda_matmul_smoke() {
    preload_torch_cuda();
    if !tch::Cuda::is_available() {
        eprintln!("CUDA not available - skipping GPU smoke test");
        return;
    }
    let n = tch::Cuda::device_count();
    eprintln!("CUDA available: {n} device(s)");
    assert!(n >= 1, "expected at least one CUDA device, got {n}");
    let dev = tch::Device::Cuda(0);
    let a = tch::Tensor::ones([64, 64], (tch::Kind::Float, dev));
    let b = tch::Tensor::ones([64, 64], (tch::Kind::Float, dev));
    let c = a.matmul(&b);
    let first = c.to_device(tch::Device::Cpu).double_value(&[0, 0]);
    assert_eq!(first, 64.0, "CUDA matmul produced wrong value");
    eprintln!("CUDA matmul ok (first={first})");
}
