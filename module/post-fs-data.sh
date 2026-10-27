#!/system/bin/sh
# post-fs-data.sh: [pstore-capture] 开机第一阶段抢救上一轮内核现场
#
# 为什么需要：/sys/fs/pstore/console-ramoops-0 完整件约 2MB，而 crash_reporter 只把 66KB
# 的 dmesg 段抄进 dropbox（SYSTEM_LAST_KMSG）后就把 pstore 整份清空。这里在 system_server
# 起来之前整份复制，保住上一轮重启的关机轨迹（sys.powerctl 一行即能定死重启调用者）。
#
# 阶段约束（KernelSU 官方文档 + Magisk 同构）：
#   1) 本阶段阻塞，最多 10 秒 —— 只做复制，不等待、不重试、不做网络
#   2) 本阶段禁止 setprop（会死锁），写属性只能用 resetprop -n；本脚本不写任何属性
#   3) 运行在 BusyBox ash standalone 模式，只用 POSIX + busybox 内建命令
#   4) 本阶段在所有模块被挂载之前，$MODDIR 是 /data/adb/modules/chiri 的真实路径
#   5) 任何异常都必须 exit 0，绝不能阻断开机
#
# 自检：无论抢到没抢到都会往 $MODDIR/pstore/capture.log 写一行结论，
#       下一轮开机看这一行即可确认时序是否如预期。

[ -z "$MODDIR" ] && MODDIR=${0%/*}

# CHIRI_PSTORE_SRC 仅用于离线自测（本机造一份假 pstore），设备上永远走 /sys/fs/pstore
PSTORE_SRC="${CHIRI_PSTORE_SRC:-/sys/fs/pstore}"
PSTORE_DST="$MODDIR/pstore"
CAP_LOG="$PSTORE_DST/capture.log"
KEEP=5
FILE_MAX_KB=8192
NEED_FREE_KB=102400

mkdir -p "$PSTORE_DST" 2>/dev/null

# [precheck] pstore 不存在或为空：本轮没有可抢救的现场
if [ ! -d "$PSTORE_SRC" ]; then
  echo "$(date '+%Y-%m-%d %H:%M:%S') SKIP pstore 目录不存在" >> "$CAP_LOG" 2>/dev/null
  exit 0
fi

LIST=$(ls -A "$PSTORE_SRC" 2>/dev/null)
if [ -z "$LIST" ]; then
  echo "$(date '+%Y-%m-%d %H:%M:%S') SKIP pstore 为空（上一轮无记录，或已被 crash_reporter 抢先清空）" >> "$CAP_LOG" 2>/dev/null
  exit 0
fi

# [budget] /data 剩余空间不足 100MB 就放弃，避免占满用户分区
AVAIL_KB=$(df -k "$MODDIR" 2>/dev/null | awk 'NR==2{print $4}')
if [ -n "$AVAIL_KB" ] && [ "$AVAIL_KB" -lt "$NEED_FREE_KB" ]; then
  echo "$(date '+%Y-%m-%d %H:%M:%S') SKIP /data 剩余空间不足（${AVAIL_KB}KB）" >> "$CAP_LOG" 2>/dev/null
  exit 0
fi

TS=$(date '+%Y%m%d-%H%M%S')
OUT="$PSTORE_DST/$TS"
mkdir -p "$OUT" 2>/dev/null || exit 0

# [copy] 逐个复制，单文件超过 8MB 跳过（console-ramoops 正常约 2MB）
COUNT=0
for f in $LIST; do
  SZ=$(wc -c < "$PSTORE_SRC/$f" 2>/dev/null)
  [ -n "$SZ" ] || continue
  [ "$SZ" -gt $((FILE_MAX_KB * 1024)) ] && continue
  cp -f "$PSTORE_SRC/$f" "$OUT/$f" 2>/dev/null && COUNT=$((COUNT + 1))
done

# [summary] 清单 + 关键字行 + 本次启动原因，纯文本便于直接 grep
{
  echo "== $TS =="
  echo "--- pstore 原始清单 ---"
  ls -l "$PSTORE_SRC" 2>/dev/null
  echo "--- 关机/重启关键行 ---"
  grep -a -i -E "powerctl|shutdown|Terminate All|sysrq|watchdog" "$OUT"/* 2>/dev/null | head -40
  echo "--- 本次启动原因 ---"
  echo "sys.boot.reason=$(getprop sys.boot.reason 2>/dev/null)"
  echo "sys.boot.reason.last=$(getprop sys.boot.reason.last 2>/dev/null)"
  echo "persist.sys.boot.reason.history=$(getprop persist.sys.boot.reason.history 2>/dev/null)"
} > "$OUT/summary.txt" 2>/dev/null

echo "$(date '+%Y-%m-%d %H:%M:%S') OK 已抢救 ${COUNT} 个文件 -> $TS" >> "$CAP_LOG" 2>/dev/null

# [rotate] 只保留最近 KEEP 份
ls -1dt "$PSTORE_DST"/*/ 2>/dev/null | tail -n +$((KEEP + 1)) | while read -r d; do
  rm -rf "$d" 2>/dev/null
done

exit 0
