#!/system/bin/sh
# pack.sh —— 外部打包工具（tar 封装 + 可选 gzip），daemon 与 WebUI 共用的对外稳定接口：
# **本文件与 core/bin/tar（外部引入的 tar 二进制，可选）不得被任何构建流程修改**
# （CI 的注释剥离已排除 scripts/ 目录，见 .github/workflows/build.yml）
# 用法：
# pack.sh archive <staging_dir> <out_tar>
# 无压缩打包（lz4 压缩由调用方 daemon 内置 lz4_flex 完成）；成功 exit 0，失败 exit 1（调用方负责保留 staging 并留痕）
# pack.sh export <src_dir> <dest_base> <fail_file> <log_file> <total_file>
# 打包为 <dest_base>.tar.gz 并删除中间 .tar（gzip 不可用时保留 .tar；区别于 logd 归档里的正式 .tar 产物）；
# 失败退出码写入 fail_file：3=源目录不可进，4=打包/压缩失败，5=源为空；exit 2=用法错误
# 调用方（WebUI）的进度源：源总字节预写 total_file（打包段分母）、<fail_file>.mode = 「压缩中」标记、
# <fail_file>.pid = 压缩器 PID（调用方读 /proc/<pid>/io 的 rchar 得压缩已读入字节，压缩结束即删）；tar/gzip 的 stderr 追加写 log_file
# tar 顺序：core/bin/tar（自带优先）→ /system/bin/tar → toybox tar → busybox tar；
# gzip 顺序：core/bin/chiri gzip（纯 Rust flate2，主选）→ 系统 gzip → busybox gzip，都没有则保留 .tar

SELF_DIR=${0%/*}
MODDIR=${SELF_DIR%/*}

# 模块二进制（工具模式：`chiri gzip <file>` 就地生成 <file>.gz 并删源），语义同 `gzip -f（见 src/logger.rs compress_cli）
CHIRI_BIN=""
if [ -x "$MODDIR/core/bin/chiri" ]; then
  CHIRI_BIN="$MODDIR/core/bin/chiri"
fi

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

GZ_BIN=""
if command -v gzip >/dev/null 2>&1; then
  GZ_BIN="gzip"
elif [ -n "$BUSYBOX" ] && "$BUSYBOX" gzip --help >/dev/null 2>&1; then
  GZ_BIN="$BUSYBOX gzip"
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
    # 清理上次异常中断的残留；.mode/.pid 必须一起删——残留 .mode 会让本轮无 gzip 时被误判「压缩中」（s:tar 不打印）、
    # 残留 .pid 会让调用方去读一个已消失进程的 /proc，两种情况都要等超时
    rm -f "$BASE.tar.part" "$BASE.tar" "$BASE.tar.gz" "$FAIL" "$FAIL.mode" "$FAIL.pid" "$PROG"
    [ -d "$SRC" ] || { echo 3 > "$FAIL"; exit 3; }
    [ -n "$(ls -A "$SRC" 2>/dev/null)" ] || { echo 5 > "$FAIL"; exit 5; }
    MODE="$FAIL.mode"
    PIDF="$FAIL.pid"
    # 有压缩工具就先把「压缩中」标记落下：tar 完成到 gzip 启动之间会有一瞬 .tar 已在、标记未建，
    # 调用方（WebUI）会把这一瞬当成「无 gzip 完成态」提前报完成；无压缩工具时不建，完成态即保留 .tar
    if [ -n "$CHIRI_BIN" ] || [ -n "$GZ_BIN" ]; then
      echo gz > "$MODE"
    fi
    if [ -n "$TOTAL" ]; then
      # 源总字节 = 打包段进度分母（打包进度不能用 tar -v 的行数：输出重定向到文件时 tar 是块缓冲，整段打包期间一行都读不到）
      # du -sk 是块对齐值、只会略大于实际字节，作分母够用；解析不到写 0，调用方退化为只有压缩段有百分比
      v=$(du -sk "$SRC" 2>/dev/null)
      k=${v%%[!0-9]*}
      [ -n "$k" ] || k=0
      echo $((k * 1024)) > "$TOTAL"
    fi
    : > "$PROG"
    $TAR_BIN -cf "$BASE.tar.part" -C "$SRC" . >> "$PROG" 2>&1 || { rm -f "$BASE.tar.part" "$MODE"; echo 4 > "$FAIL"; exit 4; }
    mv -f "$BASE.tar.part" "$BASE.tar" || { rm -f "$BASE.tar.part" "$MODE"; echo 4 > "$FAIL"; exit 4; }
    # 压缩器一律放后台跑：只为拿 PID 写进 .pid 给调用方读 /proc/<pid>/io 的 rchar（压缩段唯一的进度源），随后必须 wait 完再往下走
    if [ -n "$CHIRI_BIN" ]; then
      # 内置压缩（flate2）：chiri gzip 成功即生成 .tar.gz 并删源，设备无 gzip 二进制也能产出 .tar.gz
      "$CHIRI_BIN" gzip "$BASE.tar" 2>>"$PROG" &
      CPID=$!
      echo "$CPID" > "$PIDF"
      wait "$CPID"
      RC=$?
    elif [ -n "$GZ_BIN" ]; then
      # gzip 就地生成 .tar.gz 并删源（-f 防覆盖交互提示）
      $GZ_BIN -6 -f "$BASE.tar" 2>>"$PROG" &
      CPID=$!
      echo "$CPID" > "$PIDF"
      wait "$CPID"
      RC=$?
    fi
    if [ -n "$CHIRI_BIN" ] || [ -n "$GZ_BIN" ]; then
      rm -f "$PIDF"
      if [ "$RC" -eq 0 ]; then
        rm -f "$BASE.tar" "$MODE"
      else
        # 先写失败标记再删 .mode：反过来中间那一瞬（标记未写、标记已删）会被调用方当成 gz 完成
        echo 4 > "$FAIL"
        rm -f "$MODE"
        exit 4
      fi
    fi
    # 无 gzip：保留 .tar 作为最终产物且不写 MODE，完成态 = 「.tar 存在且无 MODE」（未压缩，用户可自行解包）
    exit 0
    ;;
  *)
    exit 2
    ;;
esac
