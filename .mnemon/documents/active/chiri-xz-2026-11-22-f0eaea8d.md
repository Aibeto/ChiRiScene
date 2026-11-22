---
id: "f0eaea8d-5cbe-4530-88f6-f4747b0fb580"
title: "ChiRi 构建/打包体积优化：XZ 打包与编译侧到顶（2026-11-22）"
description: "本地 zip 改 XZ(95) preset 6、xtask pack 入口、权限位与 config/i18n 剔除；段级 GC 与 zstd 裁剪实测 0 B 的结论与证据"
status: "active"
created_at: "2026-11-22T06:46:17.345Z"
updated_at: "2026-11-22T06:46:17.345Z"
content_hash: "4eac327aedba6b0f3e45f0d6cff1e29c5cf799db38d0e403387f84a8787e9768"
source_paths:
  - "xtask/src/main.rs"
  - "xtask/src/zip_ext.rs"
  - "scripts/build-arch/build-arch.sh"
  - "agentsdocs/02-convention.md"
  - "agentsdocs/04-hard-lessons.md"
  - "agentsdocs/07-todos.md"
  - ".dsh/docs/2026-11-22-构建打包体积优化.md"
  - "build.rs"
  - "module/customize.sh"
  - "webui/scripts/fetch-fonts.mjs"
session_ids:
  - "session-51131e41-7158-4821-a2c5-1a95fd2678be"
memory_body_ids:
  []
---

# ChiRi 构建/打包体积优化：XZ 打包与编译侧到顶（2026-11-22）

## 结论总账
- 本地模块 zip 从 Deflate 3,530,873 B → **XZ(95) 2,841,480 B（−19.5%）**，再剔除 `config/i18n` 副本 → **2,820,576 B（累计 −20.1%）**。
- 编译侧无增量：段级 GC 与 zstd default-features 裁剪实测均为 **0 B**；全模块 Full LTO 已满足。

## 打包侧（已落地，commit c38b9af + 工作区 i18n 改动）
- `xtask/src/main.rs`：`zip_method()` 默认 `CompressionMethod::Xz`，`CHIRI_ZIP_METHOD=deflate` 可退回；`zip_level()` 按算法取 **XZ=6 / Deflate=9**。preset 6 与 9 输出**同体积**（包内最大成员 chiri 约 6 MiB，撑不满 6 档的 8 MiB 字典），但 9 档设备解压多占 56 MiB，故固定 6。
- **不能写 LZMA(14)**：zip 8.6.0 `src/write.rs:2199` 对 `CompressionMethod::Lzma` 返回 `UnsupportedArchive("LZMA isn't supported for compression")`，只有 `Xz` 分支走 `lzma_rust2::XzOptions::with_preset(level)`（`src/write.rs:2202-2211`，level 0..=9）。
- `xtask pack` 子命令：打包 `output/` 下最新模块目录（跳过 `.` 开头的目录）、命名 `<目录名>-b<分秒>`、末行输出 `zip=<路径>`；`scripts/build-arch/build-arch.sh` 的 `pack_zip()` 已改为调它（此前用系统 Info-ZIP，无 xz 支持，导致本地 XZ 配置从未生效——`...-b2659.zip` 即 Deflate）。
- `xtask/src/zip_ext.rs` 的 `unix_mode()` 把磁盘 `st_mode & 0o777` 写入 zip 外部属性：zip crate 的 `FileOptions::normalize()` 默认写 0644，会丢执行位。
- `config/i18n/` 加入「只进二进制」剔除清单（与 feature.yaml、normal/{tuned_profiles,scenemode,fas}.yaml、rhine-init.yaml、normal/fas/、各 SoC feature.yaml/soc.yaml 同列）：ftl 由 `include_dir!("module/config")` 编译期嵌入（`src/common.rs:1065`），`src/i18n.rs:22` 明确不读磁盘；**仓库源文件必须保留**——`build.rs:346` 的 `assert_required_configs()` 要求存在，`webui/scripts/fetch-fonts.mjs:96` 还读 zh.ftl 做 CJK 字体子集。

## 兼容性
Magisk（busybox 1.36.1 编了 `FEATURE_UNZIP_XZ`）、KernelSU（ksud 用 zip 8 开 xz）、APatch（apd 用 zip 8.6 开 xz）均可解 method 95。KernelSU 真机已验证解包 sha256 与 0755 权限一致；Magisk/APatch 仅核过实现，未实机装机。设备端解压 XZ 228~315 ms vs Deflate 80 ms。

## 编译侧（已到顶，勿重评）
- 在位：`strip=true`、`opt-level="z"`（C 侧经 cc 映射 -Oz）、`lto="fat"`、`codegen-units=1`、`-Z build-std`（std 以 bitcode 参与 fat LTO）。
- **段级 GC 实测 0 B**：`-Z function-sections=yes` 与同链对照构建结果逐段相同（6,125,792 B、`.text` 0x264ac8、`.rodata` 0x1fdb68）；rustc 本就默认传 `-Wl,--gc-sections`，fat LTO 已在 IR 层删除死代码。踩坑：`RUSTFLAGS` 会下发给 build.rs 的 eBPF 子构建，带 `-C link-arg=-Wl,--gc-sections` 被 bpf-linker 以 `unexpected argument '-W'` 拒收。
- **zstd `default-features = false` 实测 0 B**：`cc` 的 `-ffunction-sections` + `--gc-sections` 早已丢弃 legacy 解码器与 zdict 训练器。
- 不做：ICF（函数地址相等性风险）、`--no-eh-frame-hdr`（削弱崩溃栈）、panic=abort（毁 `catch_unwind` 自愈）、`-march` 提升（28 个 SoC 不可行）、packed relocations（收益仅 −20~35 KB，且重设 RUSTFLAGS 有丢 cargo-ndk 注入 flag 的风险）。
- PGO：采样式已落地 `src/pgo.rs` + WebUI 开关，离线换算链与 `pgo`/`profile` 构建档仍在 TODO；daemon 实测约 5.7% 单核 CPU，绝对收益上限有限。

## 验证方法备忘
- python `zipfile` 不支持 method 95（XZ）解压，会抛 `NotImplementedError` —— 验包必须用 bsdtar/libarchive 或真机 busybox `unzip`。
- 比对体积/段大小必须用**同一条构建链**的对照构建：曾因官方构建嵌入旧 `webui/dist` 而误判 GC 省 4 KB。
- 完整报告：`.dsh/docs/2026-11-22-构建打包体积优化.md`。
