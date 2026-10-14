#!/usr/bin/env bash
# clean-cache.sh: [meta] [menu] [targets] [guard] [run]
# ChiRi 缓存清理: Arch 本地配合 build-arch.sh 使用
# 只动仓内白名单路径,永不向外;devimpbin/ 永不触碰
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

# [targets] 清单项格式 "路径|说明";
#   全量 = 构建缓存 + 工具链/依赖缓存;构建缓存 = 只清 .cache 与 target
#   注意: devimpbin/ 明确不入清单(用户缓存根,勿清)
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

# [guard] build-arch.sh 崩溃时原 module/ 会寄存在 trash/module.orig.*,
#         清 .cache 等于销毁唯一原件: 命中则拒绝清理并要求先手工恢复
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
        *) die "拒绝清危险路径: $p" ;;
    esac
    rm -rf -- "$p"
    c_grn "  [x] $p/"
}

# [run]
main() {
    ask MODE "清哪一类?" \
        "只清构建缓存和临时文件(.cache + target)" \
        "清所有缓存(构建 + registry + npm + webui 依赖)"
    local -a pool
    if [[ "$MODE" == 清所有* ]]; then pool=("${TARGETS_ALL[@]}"); else pool=("${TARGETS_BUILD[@]}"); fi

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
