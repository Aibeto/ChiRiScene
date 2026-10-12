---
id: "ba43ec02-61de-470c-9d56-68789fae4585"
title: "ChiRi 日志归档迁移 tar.zst：实现、zstd-sys 构建链核查与缓存清理（2026-10-12）"
description: "zstd 压缩迁移实现与构建链核查证据、遗留验证项（2026-10-12）"
status: "active"
created_at: "2026-10-12T14:31:42.672Z"
updated_at: "2026-10-12T14:31:42.672Z"
content_hash: "5140297acea3640d73e1cd675a9632284eab0db1d7a8e7e5342b14201e247815"
source_paths:
  - "Cargo.toml"
  - "src/logger.rs"
  - "src/main.rs"
  - ".github/workflows/build.yml"
  - "xtask/src/main.rs"
session_ids:
  - "638c579c-25e7-4748-a104-5653f9b37da0"
memory_body_ids:
  []
---

# ChiRi 日志归档压缩迁移：tar.lz4/gzip → tar.zst（2026-10-12）

已完成实现 + 构建链核查 + 缓存清理。11 文件工作树改动（Cargo.toml/lock、module.prop、pack.sh、src/logger.rs、src/main.rs、webui/README、export.ts、i18n en/zh、state.svelte.ts），host `cargo check` 通过（zstd-sys C 编译成功）。

## 依赖与级别口径（Cargo.toml）
- 删 `flate2`（rust_backend）与 `lz4_flex 0.14`，改 `zstd = "0.13"`（实际解析 0.13.3 → zstd-safe 7.2.4 → zstd-sys 2.0.16+zstd.1.5.7）。本仓第一个 C 依赖（vendored）。
- 级别：启动归档 level 1（快 ~250MB/s）／导出 CLI level 19（接近极限压缩率）。
- 批次键兼容历史 `.tar.lz4` / `.tar` 文件（logger.rs 合并压缩函数分流）。

## 关键 API 差异（曾致真实编译错误，已修）
- `zstd::Encoder::new` 返回 `Result<Encoder>`（flate2/lz4_flex 直接返回 Encoder）。src/logger.rs `compress_zst`（~L2918）已处理：`io::copy` 流式 + `enc.finish()`（其本身返回 io::Result，比 flate2 直接），失败删半截 dst。
- 内置压缩入口 `chiri zstd <file>`：就地生成 `<file>.zst` 并删源（main.rs [toolbox] 分发）。

## 构建链核查结论（zstd 预编译/架构问题排查）
1. **非预编译库**：crate 包内无 .a/.so；build.rs 走 `compile_zstd()`，cc crate 从 `zstd/lib` vendored C 源码现编。未开 pkg-config feature、未设 `ZSTD_SYS_USE_PKG_CONFIG`（系统 libzstd 不会被误链）。
2. **交叉架构正确**：cc-rs 按 target 切换（优先 `CC_aarch64_linux_android`）。CI 链路（.github/workflows/build.yml L145-152 NDK r29 → ANDROID_NDK_HOME → xtask/src/main.rs:199 `cargo +nightly ndk --platform 26 -t arm64-v8a build -Z build-std -r`）由 cargo-ndk 导出 NDK 的 clang CC/AR 变量，zstd-sys 产出 aarch64 ELF。
3. **既有 host 噪声（与本改动无关）**：src/monitor/screen_detect.rs:352 `libc::__system_property_get` 在 host Linux 上不存在 → `cargo check` 恒剩 1 个 E0425，真机/android 目标不受影响。
4. 陈旧缓存：曾发现 target/debug/build 里残留 Windows 主机时代 libzstd.a（archive 成员带 `E:/code/ChiRi` 路径）——**已随 target/ 整体删除**，zstd-sys 指纹干净。

## 相关裁定
- B 子代理越权项 mock-shell.ts 一行 `s:gz`→`s:zst`：**保留**（不改则本地预览永远走不到完成态，必要配套）。
- module.prop 版本 A02-04→A02-06 系用户本地先前改动，非子代理所为。

## 构建缓存清理（2026-10-12）
删 `target/`（7.9G，含 Windows 陈旧 libzstd.a）、`yumi-ebpf/target/`（229M）、`webui/node_modules/`（226M）、`webui/dist/`（776K）、scripts 两处 `__pycache__/`，共约 8.4G。未动：`devimpbin/`（15G 日志分析工作区）、各内核源码树、`git status` 12 处源码改动。下次构建需 `pnpm i` 重装 webui 依赖。

## 遗留待办
- `pnpm run type-check`：本机 node_modules 已删且原为 Windows 侧所装，需完整环境跑。
- 真机验证 `.tar.zst` 归档/导出链路。
