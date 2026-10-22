#!/usr/bin/env bash
# clean-cache.sh: [meta] [menu] [targets] [cross] [guard] [run]
# ChiRi 缓存清理, Arch 本地配合 build-arch.sh 使用,只动仓内白名单路径,devimpbin/ 永不触碰
set -euo pipefail

# [meta]
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$ROOT"

c_red()  { printf '\033[31m%s\033[0m\n' "$*"; }
c_grn()  { printf '\033[32m%s\033[0m\n' "$*"; }
c_ylw()  { printf '\033[33m%s\033[0m\n' "$*"; }
die()    { c_red "错误: $*" >&2; exit 1; }

# [menu]
ask() {
    local __var="$1" prompt="$2"; shift 2; local opts=("$@")
    { echo; c_ylw "$prompt"; } >&2
    local i
    for i in "${!opts[@]}"; do printf '  %d) %s\n' "$((i+1))" "${opts[$i]}" >&2; done
    local sel; read -rp "选择 [1-${#opts[@]}, 回车=1]: " sel || die "stdin 耗尽,无法交互(需 TTY)"
    sel="${sel:-1}"
    [[ "$sel" =~ ^[0-9]+$ ]] && (( sel>=1 && sel<=${#opts[@]} )) || die "无效选择: $sel"
    printf -v "$__var" '%s' "${opts[$((sel-1))]}"
}

du_h() { [[ -e "$1" ]] && du -sh "$1" 2>/dev/null | awk '{print $1}' || echo "-"; }

# [targets] 清单项格式 "路径|说明",全量 = 构建缓存 + 工具链/依赖缓存,构建缓存 = 只清 .cache 与 target
#   devimpbin/ 明确不入清单(用户缓存根,勿清)
TARGETS_BUILD=(
    ".cache|构建临时文件与预构建(bpf-linker/cargo-ndk/NDK r29)"
    "target|cargo 编译产物(含 -Z build-std 与 eBPF 中间)"
)
TARGETS_ALL=(
    "${TARGETS_BUILD[@]}"
    ".cargo-home|CARGO_HOME registry 源码缓存(清后需重新下载依赖)"
    ".npm-cache|npm 本地缓存(清后 npm install 重下)"
    "webui/node_modules|webui npm 依赖(清后须 cd webui && npm ci)"
)

show_plan() {  # $1=含/不含 output; 其余=清单行
    local inc_out="$1"; shift
    echo "== 将清除 =="
    local row p
    for row in "$@"; do
        IFS='|' read -r p _ <<<"$row"
        printf '  %-22s %s\n' "$p/" "$(du_h "$p")"
    done
    [[ "$inc_out" == 含 ]] && printf '  %-22s %s\n' "output/" "$(du_h output)"
    c_ylw "devimpbin/ 不在清单内,永不触碰"
}

# [cross] 交叉编译目标缓存（换工具链后 E0514 的定向修法）
#
#   `cargo --target <triple>` 会在 target/ 下建同名三元组目录。换工具链后,这些目录里的
#   .rmeta 由旧 rustc 生成,新 rustc 复用会直接报
#     E0514: found crate compiled by an incompatible version of rustc
#   典型表现是 zip/anyhow/clap 等**所有**依赖一起报错,看着像代码坏了,实为缓存作废。
#   只清这些三元组目录即可修复,host 的 target/debug 不必重建(省一次全量编译)。
cross_targets() {
    local d name
    for d in target/*/; do
        [[ -d "$d" ]] || continue
        name="${d%/}"; name="${name##*/}"
        case "$name" in debug|release|flycheck0) continue ;; esac
        [[ "$name" == *-* ]] || continue   # 只认三元组形态,防误列其它子目录
        printf 'target/%s\n' "$name"
    done
}

# 当前 rustc 二进制的安装时刻(秒)。缓存目录早于它 = 极可能由旧工具链生成
rustc_mtime() { stat -c %Y "$(rustc --print sysroot 2>/dev/null)/bin/rustc" 2>/dev/null || echo 0; }

stale_mark() {  # $1=缓存目录
    local rm dm
    rm="$(rustc_mtime)"
    [[ "$rm" -gt 0 ]] || { printf '未知'; return; }
    dm="$(stat -c %Y "$1" 2>/dev/null || echo 0)"
    if [[ "$dm" -lt "$rm" ]]; then printf '旧工具链'; else printf 'ok'; fi
}

# [guard] build-arch.sh 崩溃时原 module/ 寄存在 trash/module.orig.*,清 .cache 等于销毁唯一原件,命中则拒绝清理
guard_module_backup() {
    local b
    for b in ".cache/build-arch/trash/"module.orig.*; do
        [[ -e "$b" ]] || continue
        die "发现 build-arch 遗留的 module/ 原件: $b
     说明上次构建异常中断(module/ 现在可能是剥过注释的副本)。
     请先手工恢复: rm -rf module && mv '$b' module
     再重跑本脚本。"
    done
}

confirm_wipe() {  # $1=仓内相对路径
    local p="$1"
    [[ -e "$p" ]] || return 0
    case "$p" in  # 白名单: 只允许清单内的固定路径,杜绝变量拼接误删
        .cache|.cargo-home|.npm-cache|webui/node_modules|target|output) ;;
        target/*-*) ;;   # 交叉编译三元组目录(见 [cross]),必须含 '-',不接受 target/debug 之类
        *) die "拒绝清危险路径: $p" ;;
    esac
    rm -rf -- "$p"
    c_grn "  [x] $p/"
}

# [run]
main() {
    ask MODE "清哪一类?" \
        "只清构建缓存和临时文件(.cache + target)" \
        "清所有缓存(构建 + registry + npm + webui 依赖)" \
        "只清交叉编译目标缓存(换工具链后修 E0514,不动 host 缓存)"
    local -a pool=()
    local cross_only=0
    case "$MODE" in
        只清交叉*) cross_only=1 ;;
        清所有*)   pool=("${TARGETS_ALL[@]}") ;;
        *)         pool=("${TARGETS_BUILD[@]}") ;;
    esac

    # 交叉缓存模式: 与 output/ 无关,单独走一条更短的流程
    if (( cross_only )); then
        local -a cross=()
        mapfile -t cross < <(cross_targets)
        if (( ${#cross[@]} == 0 )); then
            c_ylw "target/ 下没有交叉编译目标缓存,无需清理"
            exit 0
        fi
        echo
        echo "== 交叉编译目标缓存 =="
        printf '  当前工具链: %s\n' "$(rustc -V 2>/dev/null || echo '读取失败')"
        local p
        for p in "${cross[@]}"; do
            printf '  %-34s %8s  %s\n' "$p/" "$(du_h "$p")" "$(stale_mark "$p")"
        done
        c_ylw "「旧工具链」= 该缓存早于当前 rustc 的安装时刻,复用必然报 E0514"
        ask GO "确认清理上述交叉缓存?" "确认" "取消"
        [[ "$GO" == 确认 ]] || { c_ylw "已取消"; exit 0; }
        for p in "${cross[@]}"; do confirm_wipe "$p"; done
        c_grn "== 清理完成 =="
        return 0
    fi

    ask WITH_OUTPUT "含 output/(模块目录与产物 zip)?" "不含" "含"
    local inc_out="${WITH_OUTPUT:0:1}"   # 取首字: 含 / 不

    guard_module_backup
    show_plan "$inc_out" "${pool[@]}"
    ask GO "确认执行?" "确认" "取消"
    [[ "$GO" == 确认 ]] || { c_ylw "已取消"; exit 0; }

    local row p
    for row in "${pool[@]}"; do
        IFS='|' read -r p _ <<<"$row"
        confirm_wipe "$p"
    done
    [[ "$inc_out" == 含 ]] && confirm_wipe output
    c_grn "== 清理完成 =="
}
main "$@"
