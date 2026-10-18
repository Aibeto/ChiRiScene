#!/system/bin/sh
# action.sh: [paths] [stop-old] [permissions] [watchdog-start] [result] 手动启动/重启调度，杀旧看门狗与主进程后重新拉起

# [paths] 定义路径与日志函数
[ -z "$MODDIR" ] && MODDIR=${0%/*}

DAEMON_PATH="$MODDIR/core/bin/chiri"
LOG_DIR="$MODDIR/logs"
LOG_FILE="$LOG_DIR/service.log"
PID_FILE="$LOG_DIR/watchdog.pid"
STOP_FLAG="$MODDIR/.uninstalling"

mkdir -p "$LOG_DIR"

# 分步消息：同时打印到 stdout（KernelSU action 弹窗可见）与日志文件
log() { echo "$(date): $*"; echo "$(date): $*" >> "$LOG_FILE"; }

# [stop-old] 终止旧看门狗与主进程，避免残留重复实例
log "stopping old watchdog and daemon..."
# 空/损坏的 pid 一律跳过：kill 0 会向整个进程组发信号
pid=$(cat "$PID_FILE" 2>/dev/null)
case "$pid" in
  ''|0|*[!0-9]*) ;;
  *) kill "$pid" 2>/dev/null ;;
esac
killall -9 chiri > /dev/null 2>&1
rm -f "$PID_FILE"
sleep 1
log "stopped."

# [permissions] 设置权限
chmod 755 "$DAEMON_PATH"

# [watchdog-start]
# [watchdog-start] 崩溃自动重启，存在 .uninstalling 或二进制被删时退出，PID 写入 watchdog.pid
# 必须 setsid + & 脱离父进程组，否则看门狗 while 不退出会拖死 action 脚本

# 检测 setsid 可用性，优先使用 BusyBox 的 setsid
SETSID_CMD=""
if command -v setsid >/dev/null 2>&1; then
  SETSID_CMD="setsid"
elif [ -x "/data/adb/magisk/busybox" ] && "/data/adb/magisk/busybox" setsid true >/dev/null 2>&1; then
  SETSID_CMD="/data/adb/magisk/busybox setsid"
elif [ -x "/data/adb/ksu/bin/busybox" ] && "/data/adb/ksu/bin/busybox" setsid true >/dev/null 2>&1; then
  SETSID_CMD="/data/adb/ksu/bin/busybox setsid"
elif [ -x "/data/adb/ap/bin/busybox" ] && "/data/adb/ap/bin/busybox" setsid true >/dev/null 2>&1; then
  SETSID_CMD="/data/adb/ap/bin/busybox setsid"
fi

# 看门狗启动命令直接走 sh -c，不经函数，确保 setsid 正常工作
WATCHDOG_CMD="sh -c '
  PIDFILE=\"\$1\"; DAEMON=\"\$2\"; FLAG=\"\$3\"
  echo \$\$ > \"\$PIDFILE\"
  BACKOFF=3
  while :; do
   [ -f \"\$FLAG\" ] && break      # 卸载标记 → 退出
   [ -f \"\$DAEMON\" ] || break    # 二进制被删 → 退出
    started=\$(date +%s 2>/dev/null)
    \"\$DAEMON\"
    ended=\$(date +%s 2>/dev/null)
    # 退出用时 <60s 判为启动即崩，退避 3→10→30→60s 封顶
    if [ -n \"\$started\" ] && [ -n \"\$ended\" ] && [ \$(( ended - started )) -lt 60 ]; then
      case \$BACKOFF in 3) BACKOFF=10 ;; 10) BACKOFF=30 ;; *) BACKOFF=60 ;; esac
    else
      BACKOFF=3
    fi
    sleep \$BACKOFF
  done
  rm -f \"\$PIDFILE\"
  exit 0
' sh \"$PID_FILE\" \"$DAEMON_PATH\" \"$STOP_FLAG\" > /dev/null 2>&1"

# 两个分支都必须 & 后台，setsid 前台执行会阻塞本脚本
if [ -n "$SETSID_CMD" ]; then
  $SETSID_CMD sh -c "$WATCHDOG_CMD" &
else
  # fallback: nohup 后台运行
  nohup sh -c "$WATCHDOG_CMD" > /dev/null 2>&1 &
fi
# 与后台作业脱钩，防止脚本退出时发 SIGHUP
disown 2>/dev/null || true

# [result] 打印执行结果
log "daemon restarted."
exit 0