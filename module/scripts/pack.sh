#!/system/bin/sh
# pack.sh —— 外部打包工具（tar 封装），daemon 与 WebUI 共用的对外稳定接口：
# **本文件与 core/bin/tar（外部引入的 tar 二进制，可选）不得被任何构建流程修改**
# （CI 的注释剥离已排除 scripts/ 目录，见 .github/workflows/build.yml）
# 用法：
# pack.sh archive <staging_dir> <out_tar>
# 无压缩打包（zstd 压缩由调用方 daemon 内置（级别 2、6 线程）完成）；成功 exit 0，失败 exit 1（调用方负责保留 staging 并留痕）
# pack.sh export <src_dir> <dest_base> <fail_file> <log_file> <total_file>
# 打包为 <dest_base>.tar（2026-10-13 起导出仅打包不压缩——zstd -19 在设备上分钟级太慢）；失败退出码写入
# fail_file：3=源目录不可进，4=打包失败，5=源为空；exit 2=用法错误
# 调用方（WebUI）的进度源：源总字节预写 total_file（打包段分母）；tar 的 stderr 追加写 log_file
# tar 顺序：core/bin/tar（自带优先）→ /system/bin/tar → toybox tar → busybox tar

SELF_DIR=${0%/*}
MODDIR=${SELF_DIR%/*}

TAR_BIN=""
if [ -x "$MODDIR/core/bin/tar" ]; then
  TAR_BIN="$MODDIR/core/bin/tar"
elif [ -x /system/bin/tar ]; then
  TAR_BIN=/system/bin/tar
elif command -v toybox >/dev/null 2>&1; then
  TAR_BIN="toybox tar"
elif [ -n "$BUSYBOX" ] && "$BUSYBOX" tar --help >/dev/null 2>&1; then
  TAR_BIN="$BUSYBOX tar"
else
  TAR_BIN="tar"
fi

case "$1" in
  archive)
    D=$2
    T=$3
    [ -d "$D" ] || exit 1
    [ -n "$T" ] || exit 1
    [ -d "${T%/*}" ] || mkdir -p "${T%/*}" || exit 1
    # .part 写入 + 原子改名：中途失败不留半截 .tar
    $TAR_BIN -cf "$T.part" -C "$D" . || { rm -f "$T.part"; exit 1; }
    mv -f "$T.part" "$T" || { rm -f "$T.part"; exit 1; }
    exit 0
    ;;
  export)
    SRC=$2
    BASE=$3
    FAIL=$4
    PROG=$5
    TOTAL=$6
    [ -n "$SRC" ] && [ -n "$BASE" ] && [ -n "$FAIL" ] && [ -n "$PROG" ] || exit 2
    # 清理上次异常中断的残留（含历史版本的中途 .zst 与标记文件）
    rm -f "$BASE.tar.part" "$BASE.tar" "$BASE.tar.zst" "$FAIL" "$FAIL.mode" "$FAIL.pid" "$PROG"
    [ -d "$SRC" ] || { echo 3 > "$FAIL"; exit 3; }
    [ -n "$(ls -A "$SRC" 2>/dev/null)" ] || { echo 5 > "$FAIL"; exit 5; }
    if [ -n "$TOTAL" ]; then
      # 源总字节 = 打包段进度分母（打包进度不能用 tar -v 的行数：输出重定向到文件时 tar 是块缓冲，整段打包期间一行都读不到）
      # du -sk 是块对齐值、只会略大于实际字节，作分母够用；解析不到写 0，调用方退化为只有压缩段有百分比
      v=$(du -sk "$SRC" 2>/dev/null)
      k=${v%%[!0-9]*}
      [ -n "$k" ] || k=0
      echo $((k * 1024)) > "$TOTAL"
    fi
    : > "$PROG"
    $TAR_BIN -cf "$BASE.tar.part" -C "$SRC" . >> "$PROG" 2>&1 || { rm -f "$BASE.tar.part"; echo 4 > "$FAIL"; exit 4; }
    mv -f "$BASE.tar.part" "$BASE.tar" || { rm -f "$BASE.tar.part"; echo 4 > "$FAIL"; exit 4; }
    # 完成态 = .tar 存在（未压缩产物，用户可自行解包）
    exit 0
    ;;
  *)
    exit 2
    ;;
esac
