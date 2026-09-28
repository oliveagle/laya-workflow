//! Compute-device resolution for the `--device` flag.
//!
//! This module is deliberately free of any `tch`/libtorch dependency so the
//! resolution rules (including the `mlx` backend and the platform gate) can be
//! unit-tested on any host, regardless of the libtorch the crate links against.
//!
//! Supported values:
//!
//! | `--device` | Behaviour |
//! |---|---|
//! | `cpu`      | Always CPU. |
//! | `cuda`     | First CUDA GPU (`cuda:0`); errors if the linked libtorch has no CUDA. |
//! | `cuda:N`   | CUDA GPU `N`; errors if unavailable or out of range. |
//! | `mlx`      | Apple MLX (Metal GPU); macOS only, errors elsewhere. |
//! | `auto`     | `mlx` on Apple Silicon when the MLX runtime is available, else CUDA when available, else CPU. |

use anyhow::{anyhow, Result};
use std::fmt;

/// The host platform, reduced to the distinction the resolver cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// An Apple platform (only these can run MLX).
    Macos,
    /// Anything else.
    Other,
}

impl Platform {
    /// The platform this binary was compiled for.
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::Macos
        } else {
            Platform::Other
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Platform::Macos => f.write_str("macos"),
            Platform::Other => f.write_str("non-macos"),
        }
    }
}

/// A fully resolved compute backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// CPU via libtorch.
    Cpu,
    /// CUDA GPU `index` via libtorch.
    Cuda(usize),
    /// Apple MLX (Metal GPU). Phase 1 executes this through the self-contained
    /// Python runtime under `laya-tch/mlx/`.
    Mlx,
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Backend::Cpu => f.write_str("cpu"),
            Backend::Cuda(i) => write!(f, "cuda:{i}"),
            Backend::Mlx => f.write_str("mlx"),
        }
    }
}

/// Runtime facts the resolver needs. Gathered from the host by the caller so the
/// pure [`resolve`] logic stays deterministic and unit-testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceEnv {
    /// Platform this process is running on.
    pub platform: Platform,
    /// Whether an importable MLX runtime is present (macOS only).
    pub mlx_available: bool,
    /// Whether the linked libtorch reports CUDA support.
    pub cuda_available: bool,
    /// Number of visible CUDA devices (0 when `cuda_available` is false).
    pub cuda_count: usize,
}

impl DeviceEnv {
    fn cuda_ok(&self, idx: usize) -> Result<()> {
        if !self.cuda_available || self.cuda_count == 0 {
            return Err(anyhow!(
                "--device cuda requested but the linked libtorch has no CUDA support. \
                 Build with `LIBTORCH=/path/to/site-packages/torch` pointing at a \
                 CUDA-enabled PyTorch install (and LIBTORCH_BYPASS_VERSION_CHECK=1 if the \
                 version differs from the one this crate expects)."
            ));
        }
        if idx >= self.cuda_count {
            return Err(anyhow!(
                "--device cuda:{idx} requested but only {} CUDA device(s) available",
                self.cuda_count
            ));
        }
        Ok(())
    }

    fn mlx_ok(&self) -> Result<()> {
        if self.platform != Platform::Macos {
            return Err(anyhow!(
                "--device mlx is only supported on macOS (Apple Silicon); the current \
                 platform ({}) has no MLX backend. Use `cpu` or `cuda` instead.",
                self.platform
            ));
        }
        if !self.mlx_available {
            return Err(anyhow!(
                "--device mlx requested but no MLX runtime is importable. Install it \
                 (`python3 -m pip install mlx`) or point LAYA_MLX_PYTHON at a Python that \
                 can `import mlx.core`."
            ));
        }
        Ok(())
    }
}

/// Resolve a `--device` value against the environment facts.
///
/// `flag` is one of `cpu`, `cuda`, `cuda:N`, `mlx` or `auto`. The `auto` rule is:
/// prefer `mlx` on Apple Silicon when the MLX runtime is available, otherwise
/// CUDA when the linked libtorch has it, otherwise CPU.
pub fn resolve(flag: &str, env: &DeviceEnv) -> Result<Backend> {
    match flag {
        "cpu" => Ok(Backend::Cpu),
        "mlx" => {
            env.mlx_ok()?;
            Ok(Backend::Mlx)
        }
        "cuda" => {
            env.cuda_ok(0)?;
            Ok(Backend::Cuda(0))
        }
        other if other.starts_with("cuda:") => {
            let idx: usize = other["cuda:".len()..].parse().map_err(|_| {
                anyhow!("invalid --device {other:?} (expected cpu | cuda | cuda:N | mlx | auto)")
            })?;
            env.cuda_ok(idx)?;
            Ok(Backend::Cuda(idx))
        }
        "auto" => {
            if env.platform == Platform::Macos && env.mlx_available {
                Ok(Backend::Mlx)
            } else if env.cuda_available && env.cuda_count > 0 {
                Ok(Backend::Cuda(0))
            } else {
                Ok(Backend::Cpu)
            }
        }
        other => Err(anyhow!(
            "unknown --device {other:?} (expected cpu | cuda | cuda:N | mlx | auto)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mac(mlx: bool) -> DeviceEnv {
        DeviceEnv {
            platform: Platform::Macos,
            mlx_available: mlx,
            cuda_available: false,
            cuda_count: 0,
        }
    }
    fn mac_cuda(mlx: bool, count: usize) -> DeviceEnv {
        DeviceEnv {
            platform: Platform::Macos,
            mlx_available: mlx,
            cuda_available: count > 0,
            cuda_count: count,
        }
    }
    fn linux(cuda: bool, count: usize) -> DeviceEnv {
        DeviceEnv {
            platform: Platform::Other,
            mlx_available: false,
            cuda_available: cuda,
            cuda_count: count,
        }
    }

    #[test]
    fn cpu_is_always_cpu() {
        assert_eq!(resolve("cpu", &mac(true)).unwrap(), Backend::Cpu);
        assert_eq!(resolve("cpu", &mac_cuda(true, 4)).unwrap(), Backend::Cpu);
        assert_eq!(resolve("cpu", &linux(true, 4)).unwrap(), Backend::Cpu);
    }

    #[test]
    fn cuda_requires_available_cuda() {
        assert_eq!(resolve("cuda", &linux(true, 2)).unwrap(), Backend::Cuda(0));
        assert_eq!(
            resolve("cuda", &mac_cuda(true, 1)).unwrap(),
            Backend::Cuda(0)
        );
        assert!(resolve("cuda", &linux(false, 0)).is_err());
        assert!(resolve("cuda", &mac(false)).is_err());
    }

    #[test]
    fn cuda_index_is_validated() {
        assert_eq!(
            resolve("cuda:0", &linux(true, 4)).unwrap(),
            Backend::Cuda(0)
        );
        assert_eq!(
            resolve("cuda:3", &linux(true, 4)).unwrap(),
            Backend::Cuda(3)
        );
        assert!(resolve("cuda:4", &linux(true, 4)).is_err(), "out of range");
        assert!(resolve("cuda:1", &linux(true, 1)).is_err());
        assert!(
            resolve("cuda:x", &linux(true, 4)).is_err(),
            "non-numeric index"
        );
        assert!(resolve("cuda:", &linux(true, 4)).is_err(), "empty index");
    }

    #[test]
    fn mlx_requires_macos_and_runtime() {
        assert_eq!(resolve("mlx", &mac(true)).unwrap(), Backend::Mlx);
        assert!(
            resolve("mlx", &mac(false)).is_err(),
            "no MLX runtime on macOS"
        );
    }

    #[test]
    fn mlx_on_non_macos_is_a_clear_error_not_cpu() {
        let err = resolve("mlx", &linux(true, 4)).unwrap_err().to_string();
        assert!(err.contains("only supported on macOS"), "message: {err}");
    }

    #[test]
    fn auto_prefers_mlx_on_apple_silicon() {
        assert_eq!(resolve("auto", &mac(true)).unwrap(), Backend::Mlx);
        assert_eq!(resolve("auto", &mac_cuda(true, 2)).unwrap(), Backend::Mlx);
    }

    #[test]
    fn auto_falls_back_without_mlx() {
        assert_eq!(resolve("auto", &mac(false)).unwrap(), Backend::Cpu);
        assert_eq!(
            resolve("auto", &mac_cuda(false, 2)).unwrap(),
            Backend::Cuda(0)
        );
        assert_eq!(resolve("auto", &linux(false, 0)).unwrap(), Backend::Cpu);
        assert_eq!(resolve("auto", &linux(true, 1)).unwrap(), Backend::Cuda(0));
    }

    #[test]
    fn unknown_device_is_rejected() {
        assert!(resolve("tpu", &mac(true)).is_err());
        assert!(resolve("", &mac(true)).is_err());
        assert!(resolve("Cpu", &mac(true)).is_err(), "case sensitive");
    }

    #[test]
    fn platform_current_matches_cfg() {
        let p = Platform::current();
        if cfg!(target_os = "macos") {
            assert_eq!(p, Platform::Macos);
        } else {
            assert_eq!(p, Platform::Other);
        }
    }

    #[test]
    fn backend_display_roundtrips() {
        assert_eq!(Backend::Cpu.to_string(), "cpu");
        assert_eq!(Backend::Cuda(2).to_string(), "cuda:2");
        assert_eq!(Backend::Mlx.to_string(), "mlx");
    }
}
