---
id: "3aaf385e-eaf7-4ff3-8a03-102d0c3fd881"
title: "ChiRi CI lint zstd-sys C 编译排查与 NDK 工具链注入修复（2026-10-13）"
description: "CI lint 裸 cargo check 缺 NDK clang 环境致 zstd-sys 编译失败的根因、唯一 C 依赖审计与 build.yml 修复（commit 590d14b）"
status: "active"
created_at: "2026-10-12T17:32:23.656Z"
updated_at: "2026-10-12T17:32:23.656Z"
content_hash: "b123eed90a8bc14937a894cadfd53a12dde0d93ee34c66aff21d7b0464a0586a"
source_paths:
  - ".github/workflows/build.yml"
  - "Cargo.toml"
  - "xtask/src/main.rs"
session_ids:
  - "ba111ff9-b85c-4c9d-a075-f652058fc0f2"
memory_body_ids:
  []
---

# ChiRi CI lint zstd-sys C 编译排查与 NDK 工具链注入修复（2026-10-13）

已修复并提交 `590d14b`（"ci(lint): zstd-sys 需注入 NDK clang 工具链环境"，.github/workflows/build.yml，+10/-1），待 push 验证。

## 问题与根因

- CI lint 步骤裸跑 `cargo check --workspace --all-targets --target aarch64-linux-android`，zstd-sys 2.0.16+zstd.1.5.7 的 build.rs（cc-rs）找不到交叉编译器而失败。
- 正式构建能过：xtask 走 `cargo ndk`，其 build_env（cargo-ndk 4.1.2 `src/cargo.rs`）注入 `CC_aarch64_linux_android`=NDK clang、CFLAGS（含 `--target=aarch64-linux-android26`）、CXX/CXXFLAGS/AR/RANLIB、`CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER` 等，cc-rs 据此取编译器。
- NDK r29 的 `toolchains/llvm/prebuilt/<ARCH>/bin` 只有 API 后缀版 `aarch64-linux-android26-clang`，**无裸名** `aarch64-linux-android-clang`，cc-rs 裸名探测必失败。

## 唯一 C 依赖审计（全 workspace）

| 依赖 | 结论 |
|---|---|
| zstd-sys 2.0.16+zstd.1.5.7 | **唯一需要 C 编译器**（vendored C 源，cc crate 现编） |
| inotify-sys 0.1.8 | build.rs 仅 BSD（netbsd/openbsd）链接 libinotify，不编 C |
| netlink-sys 0.9.0 | 无 build.rs |
| libbz2-rs-sys 0.2.2 | 纯 Rust |
| libc 0.2.190 | build.rs 不编 C |

## 修复口径（commit 590d14b）

- lint 步骤手工注入与 cargo-ndk build_env 等价的 CC/CXX/AR/RANLIB/CFLAGS（`--target=aarch64-linux-android26`）。
- 16KB 页 LDFLAGS（`-Wl,-z,max-page-size=16384`）未注入：cargo-ndk 仅在 NDK major ≤27 时追加，r28+ 默认即 16KB 页。
- 前置提交 `b968555`（"ci: lint 并入 build job，复用 NDK 环境"）：lint 不再单独 job；fmt 在 rust-cache 后、check+clippy 在 NDK 就绪后。
- lint 链路最终形态：fmt（无环境依赖，秒级）→ NDK 就绪 → check+clippy（`YUMI_SKIP_EBPF=1`，带完整 NDK 工具链环境）→ 正式构建。三层失败点（bionic 符号、zstd-sys、裸名 clang）逐一闭环。

## 证据

- cargo-ndk 4.1.2 build_env 源码：https://docs.rs/crate/cargo-ndk/4.1.2/source/src/cargo.rs （另注入 `CARGO_NDK_SYSROOT_PATH`、`CARGO_NDK_SYSROOT_LIBS_PATH`、`CARGO_NDK_SYSROOT_TARGET`、`BINDGEN_EXTRA_CLANG_ARGS_<triple>`、`_CARGO_NDK_LINK_TARGET`）
- cc_env 派生规则（源自 cc crate getenv_with_target_prefixes）：`{VAR}_{triple}` → `{VAR}_{triple_u}` → `TARGET_{VAR}` → `{VAR}`。
- 本地 registry 源码核查：`~/.cargo/registry/src/index.crates.io-*/inotify-sys-0.1.8/build.rs` 等（build.rs 有无逐一确认）。

## 关联文档

- ba43ec02-61de-470c-9d56-68789fae4585：zstd 依赖引入与构建链核查（正式构建路径，非预编译库裁定）
- 26c98c8e-12e6-48dd-9092-ce4387dcd11d：build.yml 提速候选——其中候选 G（独立快速 lint job）最终以「lint 并入 build job」落地，拍板口径以此为准
