---
id: "26c98c8e-12e6-48dd-9092-ce4387dcd11d"
title: "ChiRi nightly 工具链必要性与 build.yml 提速候选（2026-10-12）"
description: "nightly 仅 eBPF 子 crate 必需的判定与 build.yml 提速候选清单（待拍板）"
status: "active"
created_at: "2026-10-12T09:25:01.694Z"
updated_at: "2026-10-12T09:25:01.694Z"
content_hash: "f71813ccf87a3fe60f18d4788feb7e61386625a256d25d5b7266b9370910526b"
source_paths:
  - "build.rs"
  - ".github/workflows/build.yml"
session_ids:
  - "257e941c-e3c1-4dc0-9407-966d4db0543b"
memory_body_ids:
  []
---

# ChiRi nightly 工具链必要性与 CI 提速候选（2026-10-12）

## nightly 必要性判定（已完成调查）

**结论：eBPF 真编必须 nightly；主 crate stable 可过。不能整仓改 stable。**

- 证据：`build.rs` 唯一 nightly-only 调用是子 crate `yumi-ebpf` 的 `-Z build-std=core`（目标 `bpfel-unknown-none`，build.rs:137-138 附近）；主 crate `chiri` 无 `#![feature(...)]`、无 nightly-only 语法。
- 验证：stable 1.99.0 下 `YUMI_SKIP_EBPF=1 cargo check -p chiri --target aarch64-linux-android` 通过（同口径记录见文档 c1ce3257-d1be-4442-ba78-34526636507c）。
- 用法矩阵：
  | 场景 | toolchain | 环境变量 |
  |---|---|---|
  | 本地语法/类型/check | stable | `YUMI_SKIP_EBPF=1` |
  | CI 打包真机包 / 本地 release 真编 | nightly | 不设置（走 `-Z build-std` + bpf-linker） |
- `build.rs` 的 `YUMI_SKIP_EBPF` 分支仅写占位产物供 `include_bytes!` 解析，纯本地静态检查用；CI/发布不设置该变量。

## CI `build.yml` 提速候选（ROI 排序，待拍板，均未应用）

CI 现状：已用 `Swatinem/rust-cache@v2`、bpf-linker v0.11.0 / cargo-ndk v4.1.2 均经 GitHub API（带 GITHUB_TOKEN）下载预编译二进制、runner `ubuntu-26.04`、文档目录 paths-ignore。剩余短板：npm 未缓存、`npm ci` 未用、二进制下载每次重跑、WebUI 与 Rust 编译单 job 串行。

| # | 改动 | 预期节省 | 代价 |
|---|---|---|---|
| A | `actions/setup-node` 加 `cache: 'npm'` + `cache-dependency-path: webui/package-lock.json` | 1-2 min | 2 行 |
| B | `npm ci` 替代 `npm install` | 确定性、~30s | 0 |
| D | `actions/cache` 缓存 bpf-linker / cargo-ndk 二进制（key 含版本号） | 30-60s/次 | ~10 行 |
| G | 前置 stable + `YUMI_SKIP_EBPF=1 cargo check` 快速 lint job | 失败提前 3-5 min 暴露 | ~5 行 |

低优先：C（rust-cache 已最优）、E（WebUI 拆并行 job，需验证 artifact 合并）、F（`--locked`）。

低风险推荐组合 A+B+D+G，尚未获用户拍板，未改任何文件。提交文本草稿（4 拆 vs 合并）属一次性内容，未存档。

Ref: c1ce3257-d1be-4442-ba78-34526636507c（同批未提交改动的算法审查与验证口径）。
