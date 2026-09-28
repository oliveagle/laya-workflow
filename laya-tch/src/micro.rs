//! Micro-benchmark: raw linear throughput, to separate model overhead from BLAS.

use std::time::Instant;
use tch::{Kind, Tensor};

fn bench(name: &str, m: i64, k: i64, n: i64, iters: usize) {
    let x = Tensor::randn([m, k], (Kind::Float, tch::Device::Cpu));
    let w = Tensor::randn([n, k], (Kind::Float, tch::Device::Cpu));
    let mut sum = 0.0f64;
    let t = Instant::now();
    for _ in 0..iters {
        let y = x.linear(&w, None::<&Tensor>);
        sum += y.sum(Kind::Float).double_value(&[]);
    }
    let dt = t.elapsed().as_secs_f64() / iters as f64;
    let macs = m as f64 * k as f64 * n as f64;
    println!(
        "  {name:24} [{m}x{k}]x[{k}x{n}]  {:7.3} ms/iter  {:8.1} GFLOPS  (sum={:.3})",
        dt * 1000.0,
        2.0 * macs / dt / 1e9,
        sum
    );
}

fn main() {
    println!("tch threads: {}", tch::get_num_threads());
    let iters = 50;
    bench("qkv", 384, 1024, 3072, iters);
    bench("wo", 384, 1024, 1024, iters);
    bench("wi", 384, 1024, 5248, iters);
    bench("mlp_wo", 384, 2624, 1024, iters);
    bench("lin1(head)", 384, 1024, 4096, iters);
    bench("small", 96, 1024, 3072, iters);

    // elementwise / norm / softmax costs at the real shapes
    let x = Tensor::randn([384, 1024], (Kind::Float, tch::Device::Cpu));
    let w = Tensor::randn([1024], (Kind::Float, tch::Device::Cpu));
    let t = Instant::now();
    for _ in 0..iters {
        let _ = x.layer_norm(&[1024], Some(&w), None::<&Tensor>, 1e-5, false);
    }
    println!(
        "  layer_norm [384x1024]        {:7.3} ms/iter",
        t.elapsed().as_secs_f64() / iters as f64 * 1000.0
    );

    let g = Tensor::randn([384, 2624], (Kind::Float, tch::Device::Cpu));
    let t = Instant::now();
    for _ in 0..iters {
        let _ = g.gelu("none");
    }
    println!(
        "  gelu [384x2624]              {:7.3} ms/iter",
        t.elapsed().as_secs_f64() / iters as f64 * 1000.0
    );

    let s = Tensor::randn([4, 16, 96, 96], (Kind::Float, tch::Device::Cpu));
    let t = Instant::now();
    for _ in 0..iters {
        let _ = s.softmax(-1, Kind::Float);
    }
    println!(
        "  softmax [4,16,96,96]         {:7.3} ms/iter",
        t.elapsed().as_secs_f64() / iters as f64 * 1000.0
    );

    let q = Tensor::randn([4, 16, 96, 64], (Kind::Float, tch::Device::Cpu));
    let t = Instant::now();
    for _ in 0..iters {
        let _ = q.transpose(1, 2).contiguous();
    }
    println!(
        "  transpose+contig [4,16,96,64] {:7.3} ms/iter",
        t.elapsed().as_secs_f64() / iters as f64 * 1000.0
    );

    // ---- composite blocks replicating the real forward shapes ----
    let m = 384i64;
    let x0 = Tensor::randn([m, 1024], (Kind::Float, tch::Device::Cpu));
    let lnw = Tensor::randn([1024], (Kind::Float, tch::Device::Cpu));
    let wi = Tensor::randn([5248, 1024], (Kind::Float, tch::Device::Cpu));
    let wo_mlp = Tensor::randn([1024, 2624], (Kind::Float, tch::Device::Cpu));
    let t = Instant::now();
    for _ in 0..iters {
        let n = x0.layer_norm(&[1024], Some(&lnw), None::<&Tensor>, 1e-5, false);
        let gv = n.linear(&wi, None::<&Tensor>);
        let inp = gv.narrow(1, 0, 2624);
        let gt = gv.narrow(1, 2624, 2624);
        let act = inp.gelu("none") * gt;
        let _ = act.linear(&wo_mlp, None::<&Tensor>);
    }
    println!(
        "  MLP block (ln+wi+gelu+wo)       {:7.3} ms/iter",
        t.elapsed().as_secs_f64() / iters as f64 * 1000.0
    );

    // 28 distinct weight sets (emulates streaming the real encoder weights)
    let wis: Vec<Tensor> = (0..28)
        .map(|_| Tensor::randn([5248, 1024], (Kind::Float, tch::Device::Cpu)))
        .collect();
    let wos: Vec<Tensor> = (0..28)
        .map(|_| Tensor::randn([1024, 2624], (Kind::Float, tch::Device::Cpu)))
        .collect();
    let t = Instant::now();
    for _ in 0..iters {
        let mut h = x0.shallow_clone();
        for li in 0..28 {
            let n = h.layer_norm(&[1024], Some(&lnw), None::<&Tensor>, 1e-5, false);
            let gv = n.linear(&wis[li], None::<&Tensor>);
            let inp = gv.narrow(1, 0, 2624);
            let gt = gv.narrow(1, 2624, 2624);
            let act = inp.gelu("none") * gt;
            h = h + act.linear(&wos[li], None::<&Tensor>);
        }
    }
    println!(
        "  MLP x28 distinct weights        {:7.3} ms/iter (28 layers)",
        t.elapsed().as_secs_f64() / iters as f64 * 1000.0
    );

    let wqkv = Tensor::randn([3072, 1024], (Kind::Float, tch::Device::Cpu));
    let wo_attn = Tensor::randn([1024, 1024], (Kind::Float, tch::Device::Cpu));
    let cos = Tensor::randn([1, 1, 96, 64], (Kind::Float, tch::Device::Cpu));
    let sin = Tensor::randn([1, 1, 96, 64], (Kind::Float, tch::Device::Cpu));
    let mask = Tensor::zeros([4, 1, 96, 96], (Kind::Float, tch::Device::Cpu));
    let t = Instant::now();
    for _ in 0..iters {
        let qkv = x0.linear(&wqkv, None::<&Tensor>).view([4, 96, 3072]);
        let q = qkv
            .narrow(2, 0, 1024)
            .view([4, 96, 16, 64])
            .transpose(1, 2)
            .contiguous();
        let k = qkv
            .narrow(2, 1024, 1024)
            .view([4, 96, 16, 64])
            .transpose(1, 2)
            .contiguous();
        let v = qkv
            .narrow(2, 2048, 1024)
            .view([4, 96, 16, 64])
            .transpose(1, 2)
            .contiguous();
        let q = &q * &cos + &q * &sin;
        let k = &k * &cos + &k * &sin;
        let s = q.matmul(&k.transpose(-2, -1)) * 0.125 + &mask;
        let p = s.softmax(-1, Kind::Float);
        let _ = p
            .matmul(&v)
            .transpose(1, 2)
            .contiguous()
            .view([m, 1024])
            .linear(&wo_attn, None::<&Tensor>);
    }
    println!(
        "  Attn block (qkv+rope+sdpa+wo)   {:7.3} ms/iter",
        t.elapsed().as_secs_f64() / iters as f64 * 1000.0
    );
}
