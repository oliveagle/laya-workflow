//! `laya-tch` — tch-rs (libtorch / PyTorch C++ bindings) inference engine.
//!
//! The workflow runner / DSL engine has moved to the standalone
//! `laya-workflow` crate (https://github.com/oliveagle/laya-workflow).

pub mod model;

/// Load `libtorch_cuda.so` eagerly (Linux only).
///
/// torch-sys links `-ltorch_cuda`, but the GNU linker's `--as-needed` drops
/// the `DT_NEEDED` entry because our executable references no symbol from
/// `libtorch_cuda.so` directly (CUDA dispatch lives inside `libtorch_cpu.so`).
/// Without the library loaded, `at::cuda::is_available()` reports false even on
/// a CUDA-enabled build. dlopen-ing it at startup fixes that; on a CPU-only
/// build the file simply does not exist and the call is a no-op.
#[cfg(target_os = "linux")]
pub fn preload_torch_cuda() {
    unsafe {
        libc::dlopen(
            b"libtorch_cuda.so\0".as_ptr() as *const libc::c_char,
            libc::RTLD_NOW | libc::RTLD_GLOBAL,
        );
    }
}

#[cfg(not(target_os = "linux"))]
pub fn preload_torch_cuda() {}
