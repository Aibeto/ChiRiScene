#!/usr/bin/env bash
# build-arch.sh: [meta] [menu] [dl] [deps] [pkg] [toolchain] [lint] [xtask] [pack]
# ChiRi 本地构建脚本,供 Arch Linux 及衍生发行版(pacman 系)使用,与 CI(.github/workflows/build.yml) 等效,最后追加 zip 打包
# 只检查依赖、永不自动安装,缺失时交互式选「预构建拉取 / 给出包管理器命令 / 跳过」
set -euo pipefail

# [meta] 元信息与全局状态
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$ROOT"
CACHE_DIR="$ROOT/.cache/build-arch"
TRASH_DIR="$CACHE_DIR/trash"                            # 一次性废物(下载包/解压缓冲);仅本目录可随意删
mkdir -p "$TRASH_DIR"
export CARGO_HOME="${CARGO_HOME:-$ROOT/.cargo-home}"   # 仓内现成缓存,命中仓库自带 registry
BPF_LINKER_BIN=""
MISSING=()

c_red()  { printf '\033[31m%s\033[0m\n' "$*"; }
c_grn()  { printf '\033[32m%s\033[0m\n' "$*"; }
c_ylw()  { printf '\033[33m%s\033[0m\n' "$*"; }
c_dim()  { printf '\033[2m%s\033[0m\n' "$*"; }
have()   { command -v "$1" >/dev/null 2>&1; }
die()    { c_red "错误: $*" >&2; exit 1; }

# [toolchain] 通道名：唯一来源是仓库根 rust-toolchain.toml（本机与 CI 同一份）
#   读不到时回退 nightly 并**显式告警**——不静默改变语义，让人看见分叉风险
tc_channel() {
    local f="$ROOT/rust-toolchain.toml" ch
    if [[ -f "$f" ]]; then
        ch=$(sed -n 's/^[[:space:]]*channel[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$f" | head -1)
        [[ -n "$ch" ]] && { printf '%s\n' "$ch"; return; }
        c_ylw "警告: $f 里没读到 channel,回退 nightly" >&2
    else
        c_ylw "警告: 缺 $f,回退 nightly(本机与 CI 可能分叉)" >&2
    fi
    printf 'nightly\n'
}

# [dl] 下载一律带进度/速度/剩余时间,有 tty 用 curl 原生进度条,无 tty 退化为自绘的百分比+速度+ETA 文本
human_size() {  # 字节 -> 人读;numfmt 缺失时退回 awk 估算
    if have numfmt; then numfmt --to=iec --suffix=B "$1" 2>/dev/null || echo "$1 B"
    else awk -v b="$1" 'BEGIN{s=split("B KiB MiB GiB",u," ");i=1;while(b>=1024&&i<s){b/=1024;i++}printf "%.1f %s",b,u[i]}'
    fi
}

# $1=url $2=输出文件 $3=显示名
dl() {
    local url="$1" out="$2" name="${3:-$(basename "$2")}"
    echo "下载 $name"
    c_dim "  来源: $url"
    if [[ -t 1 ]]; then   # 交互终端:curl 原生进度条(进度条 + 速率 + 剩余时间)
        curl -fL --retry 3 --retry-delay 2 --connect-timeout 20 \
             --progress-bar "$url" -o "$out" || return 1
    else                  # 非终端(重定向/日志):自绘百分比+速度+ETA,避免 \r 刷屏
        local total t0 pid got now pct eta rate
        total=$(curl -fsIL --connect-timeout 20 "$url" 2>/dev/null \
                | awk 'BEGIN{IGNORECASE=1}/^content-length:/{v=$2}END{gsub(/\r/,"",v);print v}')
        t0=$(date +%s)
        curl -fL --retry 3 --retry-delay 2 --connect-timeout 20 --silent --show-error "$url" -o "$out" &
        pid=$!
        while kill -0 "$pid" 2>/dev/null; do
            sleep 2
            kill -0 "$pid" 2>/dev/null || break
            got=$(stat -c%s "$out" 2>/dev/null || echo 0)
            now=$(( $(date +%s) - t0 ))
            if [[ -n "${total:-}" && "${total:-0}" -gt 0 && "$now" -gt 0 ]]; then
                pct=$(( got * 100 / total ))
                rate=$(( got / now ))
                eta=$(( (total - got) / (rate + 1) ))
                printf '  进度 %3d%%  %s/%s  速度 %s/s  剩余约 %ss\n' \
                    "$pct" "$(human_size "$got")" "$(human_size "$total")" "$(human_size "$rate")" "$eta"
            else
                printf '  已下载 %s  已用 %ss\n' "$(human_size "$got")" "$now"
            fi
        done
        wait "$pid" || return 1
    fi
    c_grn "  -> $out ($(human_size "$(stat -c%s "$out" 2>/dev/null || echo 0)"))"
}

# [menu] 询问选项: ask VAR "提示" "选项1 选项2..."(显示序号,default=1)
ask() {
    local __var="$1" prompt="$2"; shift 2; local opts=("$@")
    { echo; c_ylw "$prompt"; } >&2   # 全走 stderr: stdout 预留给调用方捕获选项
    local i
    for i in "${!opts[@]}"; do printf '  %d) %s\n' "$((i+1))" "${opts[$i]}" >&2; done
    local sel; read -rp "选择 [1-${#opts[@]}, 回车=1]: " sel sel_junk || die "stdin 耗尽,无法交互(需 TTY)"
    sel="${sel:-1}"
    [[ "$sel" =~ ^[0-9]+$ ]] && (( sel>=1 && sel<=${#opts[@]} )) || die "无效选择: $sel"
    printf -v "$__var" '%s' "${opts[$((sel-1))]}"
}

# [deps] 依赖检查(不报不装)+ 预构建拉取,无网络时就地 IGNORE,NDK 可从 ANDROID_NDK_HOME 豁免
# 与 CI 同口径: 经 GitHub API 按模式解析 release asset(不猜文件后缀 .tgz/.tar.gz/.tar.zst),$1=owner/repo $2=tag $3=grep -E 模式
gh_asset() {
    curl -fsSL -H "Accept: application/vnd.github+json" \
        "https://api.github.com/repos/$1/releases/tags/$2" 2>/dev/null \
    | python3 -c "
import json,sys,re
pat = re.compile(sys.argv[1])
d = json.load(sys.stdin)
a = d.get('assets') or []
hit = next((x for x in a if pat.search(x['name'])), None)
if hit: print(hit['name'])
" "$3"
}

fetch_bpf_linker() {  # CI 同款 v0.11.0 官方预编译(x86_64 linux,musl 静态)
    local dir="$CACHE_DIR/bpf-linker" tmp="$TRASH_DIR/bpf-linker"; mkdir -p "$dir" "$tmp"
    if [[ ! -x "$dir/bin/bpf-linker" ]]; then
        local asset
        asset=$(gh_asset aya-rs/bpf-linker v0.11.0 '^bpf-linker-x86_64-unknown-linux-[a-z]+\.tar\.zst$' || true)
        if [[ -z "$asset" ]]; then   # API 限流/不通时的兜底(2026-10-14 实测名)
            asset="bpf-linker-x86_64-unknown-linux-musl.tar.zst"
            c_ylw "GitHub API 不可用,回退已知文件名: $asset"
        fi
        echo "下载 $asset"
        dl "https://github.com/aya-rs/bpf-linker/releases/download/v0.11.0/$asset" "$tmp/pkg.tar.zst" "$asset" \
            || die "bpf-linker 下载失败(网络?)"
        tar --use-compress-program=zstd -xf "$tmp/pkg.tar.zst" -C "$tmp"
        find "$tmp" -type f -name bpf-linker -exec install -Dm755 {} "$dir/bin/bpf-linker" \;
        rm -rf "$tmp"/*
    fi
    BPF_LINKER_BIN="$dir/bin"
    "$BPF_LINKER_BIN/bpf-linker" --version | head -1
}

fetch_cargo_ndk() {   # CI 同款 v4.1.2 官方预编译
    local dir="$CACHE_DIR/cargo-ndk" tmp="$TRASH_DIR/cargo-ndk"; mkdir -p "$dir" "$tmp"
    if [[ ! -x "$dir/bin/cargo-ndk" ]]; then
        local asset
        asset=$(gh_asset bbqsrc/cargo-ndk v4.1.2 '^cargo-ndk-x86_64-unknown-linux-gnu-v[0-9.]+\.tgz$' || true)
        if [[ -z "$asset" ]]; then   # API 限流/不通时的兜底(2026-10-14 实测名)
            asset="cargo-ndk-x86_64-unknown-linux-gnu-v4.1.2.tgz"
            c_ylw "GitHub API 不可用,回退已知文件名: $asset"
        fi
        echo "下载 $asset"
        dl "https://github.com/bbqsrc/cargo-ndk/releases/download/v4.1.2/$asset" "$tmp/pkg.tgz" "$asset" \
            || die "cargo-ndk 下载失败(网络?)"
        tar -xzf "$tmp/pkg.tgz" -C "$tmp"
        find "$tmp" -type f -name cargo-ndk -exec install -Dm755 {} "$dir/bin/cargo-ndk" \;
        rm -rf "$tmp"/*
    fi
    PATH="$dir/bin:$PATH"
    cargo ndk --version 2>&1 | head -1
}

fetch_ndk() {         # google 官方 r29 zip(约 1.7GB,CI runs-on 同源)
    local dir="$CACHE_DIR/ndk" tmp="$TRASH_DIR/ndk"; mkdir -p "$dir" "$tmp"
    if [[ ! -d "$dir/android-ndk-r29" ]]; then
        # NDK 两条路: 预构建 zip(自包含),或系统包(AUR android-ndk)
        local helper=""
        helper=$(aur_helper) || true
        if [[ -n "$helper" ]]; then
            ask NDK_WAY "NDK 怎么来?" \
                "下官方预构建 zip(~1.7GB,自包含,不动系统)" \
                "$helper 装 AUR 包 android-ndk(系统级,依赖 AUR)" \
                "跳过"
        else
            ask NDK_WAY "NDK 怎么来?" \
                "下官方预构建 zip(~1.7GB,自包含,不动系统)" \
                "跳过(未检测到 paru/yay,AUR 路线不可用)"
        fi
        if [[ "$NDK_WAY" == 跳过* ]]; then
            c_ylw "跳过 NDK;构建时 xtask 会报缺 NDK"
            return 0
        fi
        if [[ "$NDK_WAY" == *AUR* ]]; then
            "$helper" -S --needed android-ndk || die "$helper 安装 android-ndk 失败"
            [[ -d /opt/android-ndk ]] && export ANDROID_NDK_HOME="/opt/android-ndk"
            c_grn "AUR android-ndk 已装(ANDROID_NDK_HOME=${ANDROID_NDK_HOME:-未设,请自行 export})"
            return 0
        fi

        c_ylw "NDK r29 约 1.7GB,下载较久(进度/速度/剩余时间见下)"
        local url="https://dl.google.com/android/repository/android-ndk-r29-linux.zip"
        local mirror="https://mirrors.cloud.tencent.com/AndroidSDK/android-ndk-r29-linux.zip"
        ask NDK_SRC "从哪下?" "Google 官方(推荐)" "腾讯云镜像(国内快)"
        [[ "$NDK_SRC" == 腾讯云* ]] && url="$mirror"
        rm -f "$tmp/ndk.zip"
        dl "$url" "$tmp/ndk.zip" "android-ndk-r29-linux.zip" \
            || die "NDK 下载失败(网络?; 可换镜像 $mirror)"
        echo "解压中(约十几秒)..."
        local t0=$SECONDS
        rm -rf "$tmp/x"
        unzip -q "$tmp/ndk.zip" -d "$tmp/x"
        echo "  解压完成,用时 $((SECONDS - t0))s"
        mv "$tmp/x/android-ndk-r29" "$dir/android-ndk-r29"   # 原子rename,中断不留半成品
        rm -rf "$tmp"/*
    fi
    export ANDROID_NDK_HOME="$dir/android-ndk-r29"
}

fix_nightly() {
    rustup toolchain install "$(tc_channel)" --profile minimal \
        --component rust-src,clippy,rustfmt --target aarch64-linux-android \
        || die "工具链 $(tc_channel) 安装失败(rustup 网络?)"
    c_grn "工具链 $(tc_channel) 已就绪"
}

fix_android_target() {
    rustup target add aarch64-linux-android --toolchain "$(tc_channel)" \
        || die "aarch64-linux-android target 添加失败(rustup 网络?)"
    c_grn "aarch64-linux-android target 已就绪"
}

fix_rust_src() {
    echo "rustup 开始下载 rust-src(进度由 rustup 显示)"
    rustup component add rust-src clippy rustfmt --toolchain "$(tc_channel)" \
        || die "rust-src 组件安装失败(走 rustup,不属系统依赖)"
    c_grn "rust-src 已就绪"
}

fix_webui_deps() {    # lock 与 package.json 已一致(2026-10-15 补 @rolldown/binding-darwin-x64 后)；CI 亦 npm ci
    echo "npm ci(下载 115M 左右,进度/速度由 npm 显示)"
    (cd webui && npm ci --no-audit --no-fund --cache "$ROOT/.npm-cache")
}

# $1=依赖ID $2=描述 $3=修复动作(函数名)
dep_missing() {
    echo "  缺少: [$1] $2"
    MISSING+=("$1")
    local action
    ask action "处理 [$1]?" \
        "拉取预构建/本地修复(推荐,写入 $CACHE_DIR,不动系统)" \
        "暂不装,跳过本项(后果自负:构建会在对应步骤失败)" \
        "中断退出"
    if [[ "$action" == 拉取* ]]; then
        "$3"
    elif [[ "$action" == 暂不装* ]]; then
        :   # 跳过,构建在对应步骤会自然失败
    else
        exit 130
    fi
}

check_deps() {
    echo "== 依赖检查(只检不装;缺项可给命令或拉预构建) =="
    have git    || { pkg_hint "git" "拉取/版本查询基础"; die "git 缺失者请先装"; }
    have node   || dep_missing node    "Node.js(WebUI 编译)"          fetch_node
    have npm    || dep_missing npm     "npm(WebUI 依赖安装)"           fetch_node
    have zip    || dep_missing zip     "zip(最后打包必需)"              fetch_zip
    have unzip  || dep_missing unzip   "unzip(仅拉取 NDK 时需要,纯本地构建可忽略)" fetch_unzip
    have zstd   || dep_missing zstd    "zstd(解 bpf-linker 预编译包)" fetch_zstd
    have curl   || { pkg_hint "curl" "下载预构建"; die "curl 缺失者请先装"; }

    # rustup 侧的缺失由 rustup 自己补,属工具链组件,不算系统包
    have rustup || { pkg_hint "rustup" "Rust 工具链管理"; die "rustup 缺失者请先装"; }
    rustup toolchain list | grep -q "^$(tc_channel)" \
        || dep_missing nightly "nightly 工具链(CI 同款,$(tc_channel))" fix_nightly
    rustup target list --toolchain "$(tc_channel)" --installed 2>/dev/null | grep -q '^aarch64-linux-android$' \
        || dep_missing android-target "aarch64-linux-android 目标(交叉编译必需)" fix_android_target
    rustup component list --toolchain "$(tc_channel)" --installed 2>/dev/null | grep -q '^rust-src' \
        || dep_missing rust-src "nightly rust-src 组件(build-std/eBPF 必需)" fix_rust_src

    have cargo-ndk || ls "$CACHE_DIR/cargo-ndk/bin/cargo-ndk" >/dev/null 2>&1 \
        || dep_missing cargo-ndk "cargo-ndk(交叉编译入口,可用 AUR 包 cargo-ndk)" fetch_cargo_ndk
    have cargo-ndk || PATH="$CACHE_DIR/cargo-ndk/bin:$PATH"
    have bpf-linker || [[ -x "$CACHE_DIR/bpf-linker/bin/bpf-linker" ]] \
        || dep_missing bpf-linker "bpf-linker(eBPF 链接器,官方预编译)" fetch_bpf_linker
    [[ -x "$CACHE_DIR/bpf-linker/bin/bpf-linker" ]] && BPF_LINKER_BIN="$CACHE_DIR/bpf-linker/bin"

    [[ -n "${ANDROID_NDK_HOME:-}" && -d "$ANDROID_NDK_HOME" ]] \
        || [[ -d "$CACHE_DIR/ndk/android-ndk-r29" ]] \
        || [[ -n "$(find "$CACHE_DIR/ndk" -maxdepth 3 -name 'android-ndk-r*' -type d 2>/dev/null)" ]] \
        || dep_missing ndk "Android NDK(交叉链接必需;官方预编译 zip 或 AUR android-ndk)" fetch_ndk
    # 「暂不装」时不要强行指向不存在路径,留给 xtask 自己报
    if [[ -z "${ANDROID_NDK_HOME:-}" ]]; then
        local cached_ndk
        cached_ndk=$(find "$CACHE_DIR/ndk" -maxdepth 3 -name 'android-ndk-r*' -type d 2>/dev/null | sort | tail -1)
        [[ -n "$cached_ndk" ]] && ANDROID_NDK_HOME="$cached_ndk"
    fi
    [[ -n "${ANDROID_NDK_HOME:-}" ]] && export ANDROID_NDK_HOME

    [[ -d webui/node_modules ]] \
        || dep_missing webui-deps "webui/node_modules(npm 依赖)" fix_webui_deps

    if ((${#MISSING[@]})); then
        echo
        c_ylw "未装项汇总: ${MISSING[*]}"
        echo "重跑本脚本可再选「拉取预构建」;系统包按上面给出的 pacman/AUR 命令装。"
    else
        c_grn "全部依赖就绪"
    fi
}

# [pkg] 包管理器引导: 系统包永不自动装,只给可复制命令 + 认 AUR 助手
#   官方仓库已核实 zip unzip zstd nodejs npm git rustup 均在仓,android-ndk 只在 AUR,故 NDK 走「预构建 zip 或 AUR」
aur_helper() {
    local h
    for h in paru yay; do have "$h" && { echo "$h"; return 0; }; done
    return 1
}

# 打印安装指引并询问是否就地执行(默认不执行),$1=包名(空格分隔) $2=用途说明
pkg_hint() {
    local pkgs="$1" why="$2"
    local helper="" in_repo=1
    helper=$(aur_helper) || true
    [[ "$pkgs" == *android-ndk* ]] && in_repo=0   # 已核实:官方仓库没有 android-ndk

    echo "  [$pkgs] $why"
    if ((in_repo)); then
        echo "    安装(官方仓库): sudo pacman -S $pkgs"
    else
        echo "    android-ndk 仅在 AUR(官方仓库无)"
        if [[ -n "$helper" ]]; then
            echo "    安装(AUR): $helper -S $pkgs"
        else
            echo "    需先装一个 AUR 助手(paru 或 yay),或不走 AUR:改用脚本的预构建 zip 方案"
        fi
        echo "    另:也可让脚本下 Google 官方预构建 zip,不动系统"
    fi

    if [[ -n "$helper" ]]; then
        ask RUN_PM "现在就装吗?" "不装,我自己来" "现在装(需要 sudo 权限)"
        if [[ "$RUN_PM" == 现在装* ]]; then
            if ((in_repo)); then
                sudo pacman -S --needed $pkgs || die "pacman 安装 $pkgs 失败"
            else
                "$helper" -S --needed $pkgs || die "$helper 安装 $pkgs 失败"
            fi
        fi
    else
        c_dim "    未检测到 paru/yay,无法自动装 AUR 包;官方仓库包仍可自行 sudo pacman -S"
    fi
}

# 占位防呆: 基础包走 pacman(用户授权),无官方单文件预编译可拉
fetch_node()  { pkg_hint "nodejs npm" "Node.js 24 + npm(WebUI 编译;Arch 仓库版本已够)"; }
fetch_unzip() { pkg_hint "unzip"      "解 NDK 的 zip"; }
fetch_zstd()  { pkg_hint "zstd"       "解 bpf-linker 的 .tar.zst"; }
fetch_zip()   { pkg_hint "zip"        "最后打包产物"; }

# [toolchain] 工具链由仓库根的 rust-toolchain.toml 统一决定（本机与 CI 同一份）
#
# 2026-10-22 起不再用 PATH wrapper。旧做法是在 `.cache/build-arch/trash/toolchain-bin/`
# 造一批 exec nightly 真二进制的 shim 再前置 PATH，原因是本机 rustup default 是 stable，
# 而 build.rs 会 `env_remove("RUSTUP_TOOLCHAIN")` 后 spawn 裸 `cargo`，环境变量传不下去。
# 但 wrapper 必须逐个覆盖 `cargo`/`rustc`/`cargo-clippy`/`clippy-driver`/`rustfmt`，
# 漏一个就回落到 stable —— 实测漏掉 clippy 时 `cargo check` 通过、随后的 `cargo clippy`
# 拿 stable clippy-driver 读 nightly 的 .rmeta，整片报 E0514，极难定位。
# `rust-toolchain.toml` 由 rustup 自身解析（不看 PATH、不受 env_remove 影响），
# 所有入口天然一致，故整段 wrapper 删除。
#
# 本函数只做「前置自检 + 人话报错」：文件缺失或工具链没装时立刻失败，
# 而不是等到编译中途炸出一堆 E0514。
setup_toolchain() {
    [[ -f "$ROOT/rust-toolchain.toml" ]] \
        || die "缺少 $ROOT/rust-toolchain.toml（本机与 CI 的工具链唯一来源，勿删）"
    local channel; channel=$(tc_channel)
    rustup run "$channel" rustc -V >/dev/null 2>&1 || die "工具链 $channel 未安装，请执行:
      rustup toolchain install $channel --profile minimal \
          --component rust-src,clippy,rustfmt --target aarch64-linux-android"
    c_grn "工具链: $channel ($(rustup run "$channel" rustc -V | cut -d' ' -f2)) —— 由 rust-toolchain.toml 决定"
}

# [lint] 与 CI 等价的格式化/静态检查(可选,cargo fmt 不依赖 NDK)
run_lint() {
    echo "== fmt 检查 =="
    cargo fmt --all --check
    echo "== check + clippy (YUMI_SKIP_EBPF=1, NDK 环境) =="
    local nb="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin"
    env YUMI_SKIP_EBPF=1 \
        PATH="${BPF_LINKER_BIN:+$BPF_LINKER_BIN:}$PATH" \
        CC_aarch64_linux_android="$nb/clang" \
        CXX_aarch64_linux_android="$nb/clang++" \
        AR_aarch64_linux_android="$nb/llvm-ar" \
        RANLIB_aarch64_linux_android="$nb/llvm-ranlib" \
        CFLAGS_aarch64_linux_android="--target=aarch64-linux-android26" \
        bash -c 'cargo check --workspace --all-targets --target aarch64-linux-android && \
                 cargo clippy --workspace --all-targets --target aarch64-linux-android --no-deps'
}

# [strip] 注释剥离已下沉到 xtask,在 output/.temp 副本上做
#   xtask::strip_comments 在自建产物副本上剥离,module/ 全程只读,构建中断不污染工作区

# [xtask] 组装(等价 CI `--no-pack`,再自行打 zip)
run_xtask() {
    export PATH="${BPF_LINKER_BIN:+$BPF_LINKER_BIN:}$PATH"
    cargo xtask build --no-pack   # 已是 nightly(见 setup_toolchain)
}

# [pack] CI 仿名 + 分秒四位
pack_zip() {
    local dir name ts zipname
    dir=$(ls -dt output/*/ | head -n1) || die "output/ 下无模块目录,xtask 失败?"
    dir="${dir%/}"
    name=$(basename "$dir")
    ts=$(date +%M%S)
    zipname="output/${name}-b${ts}.zip"
    (cd "$dir" && zip -q -9 -r "$ROOT/$zipname" .)
    c_grn "产物: $zipname"
    c_grn "sha256: $(sha256sum "$zipname" | cut -d' ' -f1)"
}

main() {
    check_deps
    setup_toolchain

    local steps=()
    ask DO_LINT "跑 fmt+check+clippy?(CI 前置,几分钟)" "跑" "不跑"
    [[ "$DO_LINT" == 跑 ]] && steps+=(lint)
    steps+=(xtask)
    have zip && steps+=(pack)  # zip 缺失此时已被 check_deps 菜单兜底

    for s in "${steps[@]}"; do
        case "$s" in
            lint)  run_lint ;;
            xtask) run_xtask ;;
            pack)  pack_zip ;;
        esac
    done
    c_grn "== 完成 =="
}
main "$@"
