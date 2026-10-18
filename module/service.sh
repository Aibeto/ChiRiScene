#!/system/bin/sh
# service.sh: [boot-wait] [paths] [cleanup] [lab-reset] [permissions] [watchdog-start] chiri 模块启动脚本

# [boot-wait] 等待系统启动完成
until [ "$(getprop sys.boot_completed)" = "1" ]; do
  sleep 1
done

# [paths] 定义路径
[ -z "$MODDIR" ] && MODDIR=${0%/*}

DAEMON_PATH="$MODDIR/core/bin/chiri"
SCRIPTS_DIR="$MODDIR/scripts"
LOG_DIR="$MODDIR/logs"
LOG_FILE="$LOG_DIR/service.log"

mkdir -p "$LOG_DIR"

# 禁用 OPPO/OnePlus/Realme 的 Oiface
# if [ "$(getprop persist.sys.oiface.enable)" = "1" ]; then
#   setprop persist.sys.oiface.enable 0
#   echo "$(date): Oiface disabled." >> "$LOG_FILE"
# fi

# 禁用小米的 Joyose 服务
# PACKAGE_NAME="com.xiaomi.joyose"
# if pm list packages -e | grep -q "$PACKAGE_NAME"; then
# pm disable-user "$PACKAGE_NAME" >/dev/null 2>&1
# pm clear "$PACKAGE_NAME" >/dev/null 2>&1
# echo "$(date): Joyose service disabled and data cleared." >> "$LOG_FILE"
# fi

# [cleanup] 先按 pid 文件终止旧看门狗再清 daemon，否则旧看门狗 3s 后又把 daemon 拉起
if [ -f "$LOG_DIR/watchdog.pid" ]; then
  # 空/损坏的 pid 一律跳过：kill 0 会向整个进程组发信号
  pid=$(cat "$LOG_DIR/watchdog.pid" 2>/dev/null)
  case "$pid" in
    ''|0|*[!0-9]*) ;;
    *) kill "$pid" 2>/dev/null ;;
  esac
  rm -f "$LOG_DIR/watchdog.pid"
fi
killall -9 chiri > /dev/null 2>&1

# [lab-reset] 开机只删 rhine.chr，rhine-back.chr 是「上次启用过实验室」的唯一信号，勿删
rm -f "$MODDIR/rhine.chr"

# [permissions] 设置权限
chmod 755 "$DAEMON_PATH"
if [ -d "$SCRIPTS_DIR" ]; then
  chmod -R 755 "$SCRIPTS_DIR"
fi

# 调用禁用 boost 脚本（当前注释停用） if [ -f "$SCRIPTS_DIR/disable_boost.sh" ]; then echo "$(date):
# Executing disable_boost.sh" >> "$LOG_FILE""$SCRIPTS_DIR/disable_boost.sh"else echo "$(date):
# disable_boost.sh not found" >> "$LOG_FILE"fi

# [watchdog-start] daemon 崩溃自动重启，存在 .uninstalling 或二进制被删时退出，PID 写入 logs/watchdog.pid
# 必须 setsid + & 脱离父进程组，否则看门狗 while 不退出会拖死 magiskd 启动会话

# 检测 setsid 可用性，优先使用 BusyBox 的 setsid
SETSID_CMD=""
if command -v setsid >/dev/null 2>&1; then
  SETSID_CMD="setsid"
elif [ -n "$BUSYBOX" ] && "$BUSYBOX" setsid true >/dev/null 2>&1; then
  SETSID_CMD="$BUSYBOX setsid"
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
' sh \"$LOG_DIR/watchdog.pid\" \"$DAEMON_PATH\" \"$MODDIR/.uninstalling\" > /dev/null 2>&1"

# 两个分支都必须 & 后台，setsid 前台执行会阻塞本脚本
if [ -n "$SETSID_CMD" ]; then
  $SETSID_CMD sh -c "$WATCHDOG_CMD" &
else
  # fallback: nohup 后台运行
  nohup sh -c "$WATCHDOG_CMD" > /dev/null 2>&1 &
fi
# 与后台作业脱钩，防止脚本退出时发 SIGHUP
disown 2>/dev/null || true

# 方式 B: 调试模式，输出到 logs/boot_error.log
# nohup sh -c 'PIDFILE="$1"; DAEMON="$2"; FLAG="$3"; echo $$ > "$PIDFILE"; while :; do [ -f "$FLAG" ] && break; [ -f "$DAEMON" ] || break; "$DAEMON"; sleep 3; done; rm -f "$PIDFILE"; exit 0' sh "$LOG_DIR/watchdog.pid" "$DAEMON_PATH" "$MODDIR/.uninstalling" > "$LOG_DIR/boot_error.log" 2>&1 &
