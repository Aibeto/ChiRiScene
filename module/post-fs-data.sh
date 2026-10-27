#!/system/bin/sh
# ============================================================================
# [已停用] 2026-10-27 按用户要求整份注释，模块内不再执行任何 pstore 取证逻辑。
# 停用原因：取证已完成——pstore 双份完整件已拿到，四次 APEX 服务崩溃循环
# （iorapd / vendor.qsap.qapeservice / vendor.qesdk-securitymgr /
#  syshealthmon-service，每 5 秒一轮、常驻）与 IMrm HAL 缺失被确认为设备侧
#  长期故障，与 ChiRi 无关；且 boot_reasons.log 已证明重启原因是裸 reboot。
# 恢复方式：去掉下面第 1 行保留的 shebang 之外，逐行删除行首的 "# " 即可。
# 注意：module/customize.sh 里对应的 chmod 755 也已一并注释，恢复时两处一起改。
# ============================================================================
#
# 原 post-fs-data.sh: [pstore-capture] 开机第一阶段抢救上一轮内核现场
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
#
# [ -z "$MODDIR" ] && MODDIR=${0%/*}
#
# # CHIRI_PSTORE_SRC 仅用于离线自测（本机造一份假 pstore），设备上永远走 /sys/fs/pstore
# PSTORE_SRC="${CHIRI_PSTORE_SRC:-/sys/fs/pstore}"
# PSTORE_DST="$MODDIR/pstore"
# CAP_LOG="$PSTORE_DST/capture.log"
# REASON_LOG="$PSTORE_DST/boot_reasons.log"
# KEEP=5
# FILE_MAX_KB=8192
# NEED_FREE_KB=102400
#
# mkdir -p "$PSTORE_DST" 2>/dev/null
#
# # [boot-reason] 无论 pstore 有没有内容都必须执行：这是比 pstore 更可靠、更廉价的主证据源。
# # persist.sys.boot.reason.history 由 init 写入，每行 `<reason>,<时间戳秒>`，首行=本次开机。
# # 它自带真实时间戳（本阶段 RTC 尚未同步，date 输出是 1970，不可用），且能区分
# # `reboot`（无原因）与 `reboot,userrequested`（用户请求）——这正是判断"谁来重启的"关键。
# # 首行与上次相同则跳过，避免同一原因被重复记录。
# CUR_REASON=$(getprop persist.sys.boot.reason.history 2>/dev/null | head -n 1)
# if [ -n "$CUR_REASON" ]; then
#   LAST_REASON=$(tail -n 1 "$REASON_LOG" 2>/dev/null)
#   if [ "$CUR_REASON" != "$LAST_REASON" ]; then
#     echo "$CUR_REASON" >> "$REASON_LOG" 2>/dev/null
#   fi
# fi
#
# # [precheck] pstore 不存在或为空：本轮没有可抢救的现场
# if [ ! -d "$PSTORE_SRC" ]; then
#   echo "$(date '+%Y-%m-%d %H:%M:%S') SKIP pstore 目录不存在" >> "$CAP_LOG" 2>/dev/null
#   exit 0
# fi
#
# LIST=$(ls -A "$PSTORE_SRC" 2>/dev/null)
# if [ -z "$LIST" ]; then
#   echo "$(date '+%Y-%m-%d %H:%M:%S') SKIP pstore 为空（上一轮无记录，或已被 crash_reporter 抢先清空）" >> "$CAP_LOG" 2>/dev/null
#   exit 0
# fi
#
# # [budget] /data 剩余空间不足 100MB 就放弃，避免占满用户分区
# AVAIL_KB=$(df -k "$MODDIR" 2>/dev/null | awk 'NR==2{print $4}')
# if [ -n "$AVAIL_KB" ] && [ "$AVAIL_KB" -lt "$NEED_FREE_KB" ]; then
#   echo "$(date '+%Y-%m-%d %H:%M:%S') SKIP /data 剩余空间不足（${AVAIL_KB}KB）" >> "$CAP_LOG" 2>/dev/null
#   exit 0
# fi
#
# # [naming] 本阶段 RTC 尚未同步，date 输出是 1970（实测目录名 19700628-221805），不可用于命名。
# # 改用 boot-reason 首行末尾的 epoch（init 写入，真实时间），换算不出时退回原始 epoch，
# # 保证目录名始终能与 boot_reasons.log 里的记录一一对上。
# BOOT_EPOCH=$(echo "$CUR_REASON" | awk -F, 'NF>1{print $NF}')
# if [ -n "$BOOT_EPOCH" ] && [ "$BOOT_EPOCH" -gt 1600000000 ] 2>/dev/null; then
#   TS=$(date -d "@$BOOT_EPOCH" '+%Y%m%d-%H%M%S' 2>/dev/null)
#   [ -n "$TS" ] || TS="$BOOT_EPOCH"
# else
#   TS=$(date '+%Y%m%d-%H%M%S')
# fi
# OUT="$PSTORE_DST/$TS"
# mkdir -p "$OUT" 2>/dev/null || exit 0
#
# # [copy] 逐个复制，单文件超过 8MB 跳过（console-ramoops 正常约 2MB）
# COUNT=0
# for f in $LIST; do
#   SZ=$(wc -c < "$PSTORE_SRC/$f" 2>/dev/null)
#   [ -n "$SZ" ] || continue
#   [ "$SZ" -gt $((FILE_MAX_KB * 1024)) ] && continue
#   cp -f "$PSTORE_SRC/$f" "$OUT/$f" 2>/dev/null && COUNT=$((COUNT + 1))
# done
#
# # [summary] 清单 + 关键字行 + 本次启动原因，纯文本便于直接 grep
# {
#   echo "== $TS =="
#   echo "--- pstore 原始清单 ---"
#   ls -l "$PSTORE_SRC" 2>/dev/null
#   echo "--- 关机/重启关键行 ---"
#   grep -a -i -E "powerctl|shutdown|Terminate All|sysrq|watchdog" "$OUT"/* 2>/dev/null | head -40
#   echo "--- 本次启动原因 ---"
#   echo "sys.boot.reason=$(getprop sys.boot.reason 2>/dev/null)"
#   echo "sys.boot.reason.last=$(getprop sys.boot.reason.last 2>/dev/null)"
#   echo "persist.sys.boot.reason.history=$(getprop persist.sys.boot.reason.history 2>/dev/null)"
# } > "$OUT/summary.txt" 2>/dev/null
#
# echo "$(date '+%Y-%m-%d %H:%M:%S') OK 已抢救 ${COUNT} 个文件 -> $TS" >> "$CAP_LOG" 2>/dev/null
#
# # [rotate] 只保留最近 KEEP 份
# ls -1dt "$PSTORE_DST"/*/ 2>/dev/null | tail -n +$((KEEP + 1)) | while read -r d; do
#   rm -rf "$d" 2>/dev/null
# done
#
# exit 0

# [已停用] 本脚本不做任何事，直接正常退出（不阻断开机）
exit 0
