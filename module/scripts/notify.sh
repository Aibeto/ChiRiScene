#!/system/bin/sh
# notify.sh: [usage] [verdict] [ladder] [result] 通知投递的唯一外部出口，daemon 与 WebUI 共用
# 对外暴露的稳定接口，构建流程不得修改

# [usage] notify.sh <title> <tag> <body>
# tag 即 AOSP post 的第三位置参数（同 tag 互相覆盖，NOTIFICATION_ID 固定 2020）
# 三参允许为空串，但 tag 为空会让所有通知互相覆盖，脚本直接拒绝（退出码 2）

# 投递必须下沉到脚本且不看退出码，原因是 AOSP 与各家 su 的行为
# onCommand 丢掉 doNotify 的返回值并 return 0，只有权限门返回 255，故成功判据只能来自输出
# root（uid 0）直投时 callingPackage 被设成 "root"，NMS fixNotification 抛 NameNotFoundException，通知被丢弃
# com.android.shell（uid 2000）是真实包，能落地，故优先换身份投递
# MagiskSU 的 optstring 没有 l，su -lp 2000 会静默留在 root，换身份必须用 id -u 探针验明

# [verdict] 每次尝试都判定输出，判别串取自 AOSP 源码
#   posted   输出含 posted:                    → 真落地，收下
#   notfound 输出含 couldn't find notification after enqueueing → 被丢弃
#   rejected 输出含 usage: / error: invalid icon / permission denied → 旗标被拒，仍退出 0
#   unknown  以上都没有（老 ROM 不认 -v）→ 无证据可判，收下
# -v 是关键，它让 AOSP 在 enqueue 后自己回查并打印结论

# [ladder] 只保留 argv 直投 × 内层旗标档
# su 形式：su 2000 /system/bin/cmd（argv 直传，免一层 shell 解析）
# 旧实现还依次试 su 2000 -c / su shell -c / su -lp 2000 -c / su -c，
# 这些形式必须把 title/tag/body 拼进 shell 字符串，属于「远程数据 → root shell」
# 的结构性通道；转义再正确，一旦退化就是 RCE，故整条 su -c 阶梯删除。
# root（uid 0）直投本来就会被 NMS 丢弃，无 su 时才退化直投（仅兜底）
# 旗标档：-v -S bigtext → -S bigtext → 最简 -t
# 某形式 verdict=notfound 就整档跳过，去掉 -v 只会让证据消失、把失败误判成成功

# [result] stdout 逐行输出，daemon / WebUI 原样留痕
#   S<n>=<命令行> 即将尝试的策略
#   S<n>:ok | S<n>:dropped | S<n>:fail:<rc> <首行输出> 该策略结果
#   OK S<n> 收下（Rust 侧 strategy_of() 找的就是这一行的 OK 前缀）
#   VERIFY posted / unverified / dropped，LIST ok|miss|unavailable
#   FAIL <原因> 全部策略都没能落下
#   退出码：0 = 已投递（含 unverified）；1 = 全部失败/被丢弃；2 = 参数非法

TITLE=${1:-}
TAG=${2:-}
BODY=${3:-}

if [ -z "$TAG" ]; then
  echo "FAIL bad-args: tag 为空"
  exit 2
fi

# verdict：判定一次输出的成没成
verdict() {
  _o=$1
  case "$_o" in
    *"posted:"*) printf 'posted' ;;
    *"couldn't find notification after enqueueing"*) printf 'notfound' ;;
    *"usage: cmd notification post"*|*"error: invalid icon"*|*"permission denied"*) printf 'rejected' ;;
    *) printf 'unknown' ;;
  esac
}

# 反查 tag 是否真在系统通知列表里，不支持时返回 unavailable
list_check() {
  _l=$($TIMEOUT /system/bin/cmd notification list 2>/dev/null)
  if [ $? -eq 0 ] && [ -n "$_l" ]; then
    if printf '%s\n' "$_l" | grep -Fq "$TAG"; then
      printf 'ok'
    else
      printf 'miss'
    fi
  else
    printf 'unavailable'
  fi
}

# daemon 在调度线程上同步调用本脚本，每条命令都加超时，整体再受 15s 总预算约束
# 现在只有 1 种形式 × 3 档旗标，最坏 3 次 × 5s 超时，不会超过 WebUI ksu.exec 的 20s 上限
TIMEOUT=""
if command -v timeout >/dev/null 2>&1; then
  TIMEOUT="timeout 5"
fi

BUDGET=15
_T0=""
if command -v date >/dev/null 2>&1; then
  _T0=$(date +%s 2>/dev/null)
fi
# 预算用尽则返回 1，拿不到 date 就不设总预算
budget_left() {
  [ -z "$_T0" ] && return 0
  _now=$(date +%s 2>/dev/null)
  [ -z "$_now" ] && return 0
  [ $((_now - _T0)) -lt $BUDGET ]
}

# su 入口探测，各 Root 方案路径不同，找不到就只剩直投策略
SU=""
for c in /system/bin/su /system/xbin/su /system/bin/ksu /data/adb/ksu/bin/ksu /data/adb/magisk/su; do
  if [ -x "$c" ]; then SU="$c"; break; fi
done
if [ -z "$SU" ]; then
  _cand=$(command -v su 2>/dev/null)
  [ -n "$_cand" ] && SU="$_cand"
fi

N=0
ACCEPTED=""
VERDICT=""
LAST_OUT=""
DROPPED=""

# 跑一条 argv 策略并按 verdict 判定，收下 0，drop 2，被拒/失败 1
attempt() {
  _desc=$1
  shift
  N=$((N + 1))
  echo "S$N=$_desc"
  _out=$($TIMEOUT "$@" 2>&1)
  _rc=$?
  if [ "$_rc" -ne 0 ]; then
    echo "S$N:fail:$_rc $(printf '%s\n' "$_out" | head -n 1)"
    return 1
  fi
  _vd=$(verdict "$_out")
  case "$_vd" in
    posted|unknown)
      echo "S$N:ok"
      ACCEPTED=$N
      VERDICT=$_vd
      LAST_OUT=$_out
      return 0
      ;;
    notfound)
      echo "S$N:dropped"
      DROPPED=$N
      LAST_OUT=$_out
      return 2
      ;;
    *)
      echo "S$N:rejected $(printf '%s\n' "$_out" | head -n 1)"
      return 1
      ;;
  esac
}

# argv 形式是否真的切到 uid 2000，与 -v 无关，探针用 id -u，输出含 2000 即通过
# MagiskSU 没有 l，su -lp 2000 会静默留在 root，只靠 -v 分不出来
su_identity_ok() {
  _pr=$($TIMEOUT "$SU" 2000 /system/bin/id -u 2>/dev/null)
  case "$_pr" in
    *2000*) return 0 ;;
  esac
  N=$((N + 1))
  echo "S$N=[probe] su 未真正切到 uid 2000（id -u 输出：$(printf '%s' "$_pr" | head -n 1)），跳过 argv 形式"
  return 1
}

# 外层只保留 argv 直投；无 su 时退化为单一直投
if [ -n "$SU" ]; then
  FORMS="argv"
else
  FORMS="direct"
fi

BUDGET_HIT=""
for _form in $FORMS; do
  [ -n "$ACCEPTED" ] && break
  if ! budget_left; then BUDGET_HIT=1; break; fi
  # argv 形式先验身份，探针不通过就失败（不再有 su -c 备选）
  if [ "$_form" = "argv" ]; then
    su_identity_ok || continue
  fi
  for _fv in 0 1 2; do
    if ! budget_left; then BUDGET_HIT=1; break 2; fi
    case "$_fv" in
      0) FLAGS="-v -S bigtext"; VLABEL="verbose+bigtext" ;;
      1) FLAGS="-S bigtext";    VLABEL="bigtext" ;;
      *) FLAGS="";              VLABEL="minimal" ;;
    esac
    # FLAGS 是自有定值需分词故不加引号；title/tag/body 按 argv 位置直传，不经 shell 解析
    case "$_form" in
      argv)   attempt "[$VLABEL] su 2000 /system/bin/cmd notification post $FLAGS -t <title> <tag> <body>" \
                "$SU" 2000 /system/bin/cmd notification post $FLAGS -t "$TITLE" "$TAG" "$BODY" ;;
      direct) attempt "[$VLABEL] cmd notification post $FLAGS -t <title> <tag> <body>（无 su，直投）" \
                /system/bin/cmd notification post $FLAGS -t "$TITLE" "$TAG" "$BODY" ;;
    esac
    _r=$?
    [ "$_r" -eq 0 ] && break
    # dropped：该 su 形式没有真换身份，去掉 -v 只会把失败误判成 unknown，故整档跳过
    [ "$_r" -eq 2 ] && break
  done
done

if [ -z "$ACCEPTED" ]; then
  if [ -n "$BUDGET_HIT" ]; then
    echo "FAIL 总预算 ${BUDGET}s 用尽，未找到可用策略（su=${SU:-未找到}）"
    exit 1
  fi
  # 所有策略都被丢弃时最后反查一次，-v 的回查有 500ms×3 轮询窗口
  _lc=$(list_check)
  if [ "$_lc" = "ok" ]; then
    echo "OK list"
    echo "LIST ok $TAG"
    exit 0
  fi
  if [ -n "$DROPPED" ]; then
    echo "VERIFY dropped"
    echo "FAIL 投递被丢弃：cmd 接受了但 AOSP 回查不到（uid 0 直投或 su 未真换身份），list=$_lc"
  else
    echo "FAIL 所有策略均失败（su=${SU:-未找到}）"
  fi
  exit 1
fi

# 先打哪条策略被接受，Rust 侧 strategy_of() 找的就是这一行的 OK 前缀
echo "OK S$ACCEPTED"

# 再报告验证结论，posted 最可信，unknown 说明老 ROM 不认 -v，再用 list 补一次
case "$VERDICT" in
  posted) echo "VERIFY posted" ;;
  *)
    _lc=$(list_check)
    case "$_lc" in
      ok)   echo "LIST ok $TAG" ;;
      miss) echo "VERIFY unverified（老 ROM 不认 -v），list=miss $TAG" ;;
      *)    echo "VERIFY unverified（老 ROM 不认 -v）" ;;
    esac
    ;;
esac

exit 0
