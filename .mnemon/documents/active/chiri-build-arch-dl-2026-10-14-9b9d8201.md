---
id: "9b9d8201-877a-406a-85cd-7e3962c14e46"
title: "ChiRi build-arch 三项增强交付：下载进度 dl() / 包管理引导 / 脱敏（2026-10-14）"
description: "build-arch.sh 下载进度 dl()、pacman/AUR 引导、脱敏、selftest 47/47 与真实构建冒烟交付记录"
status: "active"
created_at: "2026-10-14T06:42:08.147Z"
updated_at: "2026-10-14T06:42:08.147Z"
content_hash: "222012e0703129141b833b9cfb158881d24ba4cd3ad6c5d738ba8f12d3b64e3e"
source_paths:
  - "scripts/build-arch/build-arch.sh"
  - "scripts/build-arch/clean-cache.sh"
  - "scripts/build-arch/README.md"
  - "devimpbin/selftest-build-arch.sh"
session_ids:
  - "7a27ebc1-1f54-40e3-bcea-46d5c504a196"
memory_body_ids:
  []
---

# ChiRi build-arch 三项增强交付：下载进度 / 包管理引导 / 脱敏（2026-10-14）

## 交付范围（三项用户要求全部完成，并实跑通一次完整构建冒烟）

文件：`scripts/build-arch/build-arch.sh`（446 行）、`scripts/build-arch/clean-cache.sh`（105 行）、`scripts/build-arch/README.md`（90 行）；自测 `devimpbin/selftest-build-arch.sh` 从 37 项扩到 **47 pass / 0 fail**。临时文件 `_smoke-lib.sh` 已清；`devimpbin/_tmp`、`_x1009` 等为用户本职工作产物，未动。git 状态：`.gitignore` 修改 + `scripts/build-arch/` 未跟踪。

## 1. 下载进度：dl() 唯一下载入口

- 新增 `dl()`，三处下载（bpf-linker、cargo-ndk、NDK）全部改走它，不再裸调 curl；自测含「下载必须走 dl()」断言。
- tty 交互 → curl 原生进度条；非 tty（重定向/日志）→ 自绘进度行，避免 `\r` 刷屏。实测：`进度   7%  172KB/2.4MB  速度 86KB/s  剩余约 26s`。
- 字节数 `numfmt` 转人读，无 numfmt 时退 awk。`fix_rust_src` 原 `>/dev/null` 吞进度已去掉；NDK 解压加计时；npm/rustup 进度由其自身打印。

## 2. 包管理器引导：pkg_hint（先核实 pacman -Si）

- 官方仓库均有：`zip unzip rsync zstd perl nodejs npm git rustup`；**`android-ndk` 官方仓库没有，仅 AUR**（r30-1，283 票）。
- `pkg_hint` 缺包时打印可复制命令并询问是否就地执行（默认不装）；检测到 `paru`/`yay` 走 AUR 助手，未检测到提示先装助手或改用预构建 zip。
- NDK 双来源：Google 官方预构建 zip（可选腾讯镜像）或 AUR 助手装系统包。修过一处文案矛盾：NDK 走 AUR 分支却打印「安装(官方仓库): sudo pacman -S android-ndk」。
- `nightly` 工具链与 `aarch64-linux-android` target 原先仅一句报错即退，现可由脚本经 rustup 补装（豁免 pacman）。

## 3. 脱敏（两处本机痕迹，已清）

- build-arch.sh 第 3 行 `CachyOS/Manjaro/EndeavourOS` → 中性「Arch Linux 及其衍生发行版(pacman 系)」。
- selftest 第 4 行硬编码 `cd /home/aibeto/code/ChiRi` → 从 `BASH_SOURCE` 推仓库根（DSH bash 下 source 会丢 BASH_SOURCE 解析，沙箱 smoke 需仓内 harness）。
- 复查断言（进 selftest）：无 `/home/`、用户名、`ghp_`、`AKIA`、`PRIVATE KEY`、内网 IP；无 em/en dash 标点（`--` shell 参数不算）。注意：selftest 断言文件本身含这些敏感词样例，故 grep 排除只对被检脚本。

## 4. 真实构建冒烟（除 NDK 1.7G 下载外全链路验证）

依赖检查通过 → 工具链钉 nightly（`1.101.0-nightly`）→ 副本剥注释生效 → xtask 编译 WebUI（在 head 截断处主动中断）。事后验证：`module/` 被 trap 还原（注释 20 行回原值）、无 `module.orig` 遗留、无残留进程。附带发现：`cargo check` 报 `xtask/Cargo.toml:8` unused dependency `chrono`（未处理）。

## 5. 附带修复：clean-cache.sh `local -n` 行为 bug

- `local -n` + 写穿赋值致清理清单错乱，模式 2 只删了 target 和 output；改普通数组，沙箱实测正确。
- 新增守护：`.cache/build-arch/trash/module.orig.*` 残留（上次构建异常中断，module/ 可能是剥过注释的副本）时**拒绝清理**并给恢复命令——与 build-arch.sh 的 `MODULE_BACKUP="$TRASH_DIR/module.orig.$$"` + `trap restore_module EXIT` 机制呼应。

## 关联文档

- 5ff5d43d-27d6-4c0b-8a83-cfdbd45b68a7：-Z nightly 通道根因与 setup_toolchain() wrapper 修法（本次是其后续增强）。
- 26c98c8e-12e6-48dd-9092-ce4387dcd11d：nightly 仅 eBPF 必需判定与 CI 提速候选。
- ba43ec02-61de-470c-9d56-68789fae4585：临时 Rust 工程 `[workspace]` 隔离事故教训（冒烟 harness 沿用该经验，放在仓内而非 /tmp）。
