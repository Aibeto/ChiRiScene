---
id: "5ff5d43d-27d6-4c0b-8a83-cfdbd45b68a7"
title: "ChiRi 本地构建 -Z nightly 通道根因与 build-arch.sh 工具链 wrapper 修复（2026-10-14）"
description: "build.rs:150 env_remove(RUSTUP_TOOLCHAIN) 致最内层 eBPF cargo 回落 stable 拒绝 -Z 的根因、PATH 前置 wrapper 修法与 37/37 自测"
status: "active"
created_at: "2026-10-14T06:23:48.212Z"
updated_at: "2026-10-14T06:23:48.212Z"
content_hash: "8cc4fc6f64ba880571b98e7e9d06d8859da10a526fedee4bbf81bfdbeb262435"
source_paths:
  - "scripts/build-arch/build-arch.sh"
  - "scripts/build-arch/README.md"
  - "build.rs"
  - "xtask/src/main.rs"
  - "devimpbin/selftest-build-arch.sh"
session_ids:
  - "5bc16e60-2f90-4dc8-860b-01b00b1df630"
memory_body_ids:
  []
---

# ChiRi 本地构建 -Z nightly 通道根因与 build-arch.sh 工具链 wrapper 修复（2026-10-14）

## 根因（调查完成）

- 调用链：`scripts/build-arch/build-arch.sh` → `cargo +nightly xtask build --no-pack` → xtask（`xtask/src/main.rs:199`：`cargo +nightly ndk --platform 26 -t arm64-v8a build -Z build-std -r`）→ cargo-ndk → build.rs → **最内层** `Command::new("cargo")`（build.rs:146）编 eBPF 子 crate（`bpfel-unknown-none`，`-Z build-std=core`）。
- 真凶：**build.rs:150 `.env_remove("RUSTUP_TOOLCHAIN")`** —— 主动清掉工具链覆盖变量，使最内层裸 `cargo` 经 rustup shim 回落到 rustup default。本机 default 是 `stable-x86_64-unknown-linux-gnu` → 报错逐字为：
  `error: the -Z flag is only accepted on the nightly channel of Cargo, but this is the stable channel`
- CI 不炸的原因：`dtolnay/rust-toolchain@nightly` 把 runner 的 rustup **default** 设为 nightly，环境变量被删也不受影响。故 `RUSTUP_TOOLCHAIN=nightly` 等环境变量方案对本脚本无效（会被 build.rs 主动移除）。
- 本机事实（2026-10-14）：无 rust-toolchain 文件；default stable；无 override；shim `/home/aibeto/.cargo/bin/cargo`；nightly 真实 cargo `/home/aibeto/.rustup/toolchains/nightly-x86_64-unknown-linux-gnu/bin/cargo`。

## 修法（已实施）

- `build-arch.sh` 新增 `setup_toolchain()`：在 `.cache/build-arch/trash/toolchain-bin/` 生成 `cargo`/`rustc`/`rustdoc` 三个 wrapper —— 剥掉 rustup 专有的 `+nightly` 参数（`[ "$1" = "+nightly" ] && shift`）后直接 exec nightly 真实二进制，并把该目录**前置 PATH**。等效复刻 CI「default = nightly」语义：不改 rustup 全局设置、不改仓库源码、脚本退出即失效。
- 配套：lint/xtask 里 `cargo +nightly …` 改为裸 `cargo …`（PATH 首已是 nightly 真实二进制，不认 rustup shim 的 `+` 语法）。
- 否决的替代方案：`rustup override set nightly` —— 沙箱内 `~/.rustup/settings.toml` 只读（os error 30）且会持久污染用户全局配置；改 rust-toolchain.toml 会影响仓库所有人与 CI 行为。

## 决定性验证

- 独立小 crate（空 `[workspace]` 表隔离，避免被根 workspace 吸收；呼应 ba43ec02 事故教训）复刻 build.rs 调用环境：无 wrapper 报上述 stable 通道错误（逐字一致），有 wrapper 则 `-Z build-std=core` 正常编译 core/compiler_builtins。
- `env -u RUSTUP_TOOLCHAIN` 后裸 `cargo` 解析 nightly ✓；`cargo --version` 会短路跳过 `-Z` 校验，不能作验证手段。
- 自测 `devimpbin/selftest-build-arch.sh` 扩到 **37 pass / 0 fail**（新增 -Z 通道双断言、兜底名断言、限流自动 skip）。教训：脚本内 `set -e` 下子 shell 取 rc 须用 `( … ) || rc=$?` 而非 `( … ); rc=$?`。

## 附带修复：GitHub API 限流兜底

- 未认证 GitHub API 限流 60 次/时（实测撞 `403 / 0-of-60`）；原实现遇限流直接 die。已加**实测过的固定文件名兜底**：bpf-linker `-musl.tar.zst`、cargo-ndk `.tgz`，API 不通自动回退并提示，不再中断。

## 关联文档

- 26c98c8e-12e6-48dd-9092-ce4387dcd11d：nightly 仅 eBPF 必需的判定与本地/CI 用法矩阵（本次修复的判定前提）。
- 3aaf385e-eaf7-4ff3-8a03-102d0c3fd881：CI lint NDK 工具链注入（cc-rs 裸名 clang 根因，另一条构建链）。
- ba43ec02-61de-470c-9d56-68789fae4585：临时 Rust 工程须空 `[workspace]` 隔离的事故教训（本次验证方法沿用）。
