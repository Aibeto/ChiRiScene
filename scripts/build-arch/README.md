# build-arch: Arch Linux 本地构建脚本

跟 `.github/workflows/build.yml` 等价。产物 `output/<CI基名>-b<MMSS>.zip` 解压后与 CI 一致。zip 名取 CI 基名(`module.prop` 的 `name-version-git提交数`),末尾的 `-b<分秒四位>` 用来区分同一分钟内的多次构建。

## 用法

```bash
# 仓库根
bash scripts/build-arch/build-arch.sh     # 构建 + 打包
bash scripts/build-arch/clean-cache.sh    # 清理缓存(交互选择范围)
```

`build-arch.sh` 会依次问:缺哪个依赖怎么处理、要不要跑 fmt+check+clippy。NDK 有 1.7G,单独问来源。

### clean-cache.sh

两档范围,产物另算,只删仓库内的白名单路径:

| 范围 | 内容 |
|---|---|
| 只清构建缓存 | `.cache/`(临时文件+预构建)、`target/` |
| 清所有缓存 | 上一行 + `.cargo-home/`、`.npm-cache/`、`webui/node_modules/` |
| 可选 | `output/`(模块目录与产物 zip) |

`devimpbin/` 永不触碰,路径走白名单 case。如果 `.cache/.../trash/module.orig.*` 还在(说明上次构建异常中断,`module/` 可能是剥过注释的副本),脚本拒绝清理并给出恢复命令。清掉 `.cargo-home`/`.npm-cache`/`webui/node_modules` 后依赖要重下,所以只在「清所有缓存」里出现。

## 依赖怎么补

三类,界限按「动不动系统」划:

| 类别 | 例子 | 处理 |
|---|---|---|
| 系统包 | git/perl/curl/rustup/zip/unzip/zstd/rsync/nodejs | 给 `pacman -S` 命令,问你要不要现在装,默认不装 |
| rustup 组件 | nightly 工具链、aarch64 目标、rust-src | 交互确认后走 `rustup`,算工具链不算系统包 |
| 官方预构建 | bpf-linker、cargo-ndk、NDK | 下到 `.cache/build-arch/`,不碰系统 |

系统包一律不自动装,但会把命令写清楚,并提供「现在装」选项(需要 sudo 的那些包用 `sudo pacman`;`android-ndk` 官方仓库没有,只有在 AUR,脚本检测到 `paru`/`yay` 时改走 AUR 助手)。下载全部带进度、速度、剩余时间。

## 下载进度

所有下载都过 `dl()`,不裸调 curl:

- 交互终端:交给 curl 原生进度条(含速率与剩余时间)
- 重定向/日志:自绘 `进度 37%  620KB/2.4MB  速度 86KB/s  剩余约 26s`,避免 `\r` 刷屏

## 临时文件

一次性文件(下载包、解压缓冲、module 副本)全进 `.cache/build-arch/trash/`,仓库根的 `.cache/` 已在 `.gitignore` 里。半成品不留:NDK 解到 `trash/ndk/x` 再原子 `mv`。

## 跟 CI 的差别

只在行为层有差别,产物不变:

| 项 | CI | 本地 |
|---|---|---|
| 依赖 | 自动 apt/binstall | 只检不装,菜单选预构建或包管理器命令 |
| lint | 强制 | 菜单可选 |
| `module/` 注释剥离 | 改 CI 工作区 | 副本剥,真身先 `mv` 到 `trash/module.orig.$$`,`trap EXIT` 自动还原 |
| 打包 | GitHub 上传时压 zip | `zip -q -9 -r`,产物名追加 `-b<MMSS>` |
| cargo 缓存 | runner 缓存 | `CARGO_HOME=仓内 .cargo-home` |
| nightly | 每日解析 | PATH 前置 nightly wrapper,全程钉死;漂移时 `rustup update nightly` |

## 为什么必须钉工具链

`cargo +nightly xtask build` 不够用:

1. `+nightly` 只作用于外层 cargo;
2. `build.rs:150` 会 `env_remove("RUSTUP_TOOLCHAIN")` 再 spawn 裸 `cargo` 编 eBPF;
3. 这个内层 cargo 经 rustup shim 回落到 default 通道(本机是 stable),`-Z build-std=core` 直接报错:

```
error: the `-Z` flag is only accepted on the nightly channel of Cargo,
       but this is the `stable` channel
```

CI 没这个问题,因为 `dtolnay/rust-toolchain@nightly` 把 runner 的 rustup default 设成了 nightly,环境变量被删也不影响。

`setup_toolchain()` 的做法:在 `.cache/build-arch/trash/toolchain-bin/` 生成 `cargo`/`rustc`/`rustdoc` 三个 wrapper(剥掉 rustup 专有的 `+nightly` 语法后 exec nightly 真实二进制),再把这个目录前置到 PATH。等于在本地复刻「default 就是 nightly」,不改 rustup 全局设置、不动仓库源码,脚本退出即失效。

## 预构建拉取

asset 名经 GitHub API 解析,不写死(上游后缀变过):

| 工具 | 版本 | asset | 落点 |
|---|---|---|---|
| bpf-linker | v0.11.0 | `bpf-linker-x86_64-unknown-linux-musl.tar.zst` | `.cache/build-arch/bpf-linker/bin/` |
| cargo-ndk | v4.1.2 | `cargo-ndk-x86_64-unknown-linux-gnu-v4.1.2.tgz` | `.cache/build-arch/cargo-ndk/bin/` |
| Android NDK | r29 | `android-ndk-r29-linux.zip`(约 1.7G) | `.cache/build-arch/ndk/` |

GitHub 未认证 API 限流是 60 次/时,撞上了会自动回退到上表的固定文件名,不中断。也可以手工下好放进对应落点,脚本看到可执行文件就跳过下载。
