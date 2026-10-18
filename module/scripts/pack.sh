#!/system/bin/sh
# pack.sh: [archive] [export] 外部打包工具，daemon 与 WebUI 共用的对外稳定接口
# 本文件与 core/bin/tar 不得被任何构建流程修改，CI 注释剥离已排除 scripts/ 目录 (.github/workflows/build.yml)
# pack.sh archive <staging_dir> <out_tar>  无压缩打包，成功 0 失败 1
# pack.sh export <src_dir> <dest_base> <fail_file> <log_file> <total_file>  只打包不压缩
# export 失败码写 fail_file：3=源目录不可进 4=打包失败 5=源为空，exit 2=用法错误
# tar 顺序：core/bin/tar → /system/bin/tar → toybox tar → busybox tar

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
    # .part 写入 + 原子改名，中途失败不留半截 .tar
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
    # 清理上次异常中断的残留
    rm -f "$BASE.tar.part" "$BASE.tar" "$BASE.tar.zst" "$FAIL" "$FAIL.mode" "$FAIL.pid" "$PROG"
    [ -d "$SRC" ] || { echo 3 > "$FAIL"; exit 3; }
    [ -n "$(ls -A "$SRC" 2>/dev/null)" ] || { echo 5 > "$FAIL"; exit 5; }
    if [ -n "$TOTAL" ]; then
      # 源总字节 = 打包段进度分母，tar -v 行数不可用（重定向时块缓冲，整段期间读不到）
      # du -sk 是块对齐值，解析不到写 0
      v=$(du -sk "$SRC" 2>/dev/null)
      k=${v%%[!0-9]*}
      [ -n "$k" ] || k=0
      echo $((k * 1024)) > "$TOTAL"
    fi
    : > "$PROG"
    $TAR_BIN -cf "$BASE.tar.part" -C "$SRC" . >> "$PROG" 2>&1 || { rm -f "$BASE.tar.part"; echo 4 > "$FAIL"; exit 4; }
    mv -f "$BASE.tar.part" "$BASE.tar" || { rm -f "$BASE.tar.part"; echo 4 > "$FAIL"; exit 4; }
    # 完成态 = .tar 存在
    exit 0
    ;;
  *)
    exit 2
    ;;
esac
